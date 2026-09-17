#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use authz_resolver_sdk::{
    AuthZResolverApi, PolicyEnforcer,
    constraints::{Constraint, EqPredicate, Predicate},
    models::{EvaluationRequest, EvaluationResponse, EvaluationResponseContext},
};
use base64::Engine as _;
use file_parser::domain::local_client::FileParserLocalClient;
use file_parser::domain::service::{FileParserService, ServiceConfig as FileParserServiceConfig};
use file_parser::infra::parsers::PlainTextParser;
use file_parser_sdk::FileParserClientV1;
use prompt_scanner_sdk::{
    Attachment, BackendOutcome, ContentKind, Finding, PromptScannerError, ScanInput, ScanRequest,
    ScannerBackendClient, ScannerBackendSpecV1, Severity, TenantPolicy, VerdictAction,
};
use sea_orm_migration::MigratorTrait;
use toolkit::api::canonical_prelude::CanonicalError;
use toolkit::client_hub::{ClientHub, ClientScope};
use toolkit::gts::PluginV1;
use toolkit_db::migration_runner::run_migrations_for_testing;
use toolkit_db::{ConnectOpts, DBProvider, connect_db};
use toolkit_security::{PlatformSecurityContext, SecurityContext, pep_properties};
use types_registry_sdk::TypesRegistryClient;
use types_registry_sdk::testing::{MockTypesRegistryClient, make_test_instance};
use uuid::Uuid;

use crate::infra::storage::OrmScanRecordRepository;
use crate::infra::storage::migrations::Migrator;

use super::{DbProvider, Service};

type TestService = Service<OrmScanRecordRepository>;

/// Permissive PDP fake: permits every request, scoped to the caller's own
/// tenant via an `Eq(owner_tenant_id, ctx.subject_tenant_id())` constraint.
/// Sufficient to exercise tenant isolation without a real `AuthZ` backend.
struct MockAuthZResolver;

#[async_trait]
impl AuthZResolverApi for MockAuthZResolver {
    async fn evaluate(
        &self,
        _ctx: PlatformSecurityContext,
        request: EvaluationRequest,
    ) -> Result<EvaluationResponse, CanonicalError> {
        let tenant_str = request
            .subject
            .properties
            .get("tenant_id")
            .and_then(serde_json::Value::as_str)
            .expect("MockAuthZResolver: subject.properties[tenant_id] missing");
        let tenant = Uuid::parse_str(tenant_str).expect("MockAuthZResolver: bad tenant UUID");

        Ok(EvaluationResponse {
            decision: true,
            context: EvaluationResponseContext {
                constraints: vec![Constraint {
                    predicates: vec![Predicate::Eq(EqPredicate::new(
                        pep_properties::OWNER_TENANT_ID,
                        tenant,
                    ))],
                }],
                deny_reason: None,
            },
        })
    }
}

fn mock_enforcer() -> PolicyEnforcer {
    PolicyEnforcer::new(Arc::new(MockAuthZResolver))
}

fn ctx_for_tenant(tenant_id: Uuid) -> SecurityContext {
    SecurityContext::builder()
        .subject_id(Uuid::new_v4())
        .subject_tenant_id(tenant_id)
        .build()
        .expect("valid SecurityContext")
}

async fn in_memory_db() -> Arc<DbProvider> {
    let opts = ConnectOpts {
        max_conns: Some(1),
        min_conns: Some(1),
        ..Default::default()
    };
    let db = connect_db("sqlite::memory:", opts)
        .await
        .expect("connect in-memory sqlite");
    run_migrations_for_testing(&db, Migrator::migrations())
        .await
        .map_err(|e| e.to_string())
        .expect("run migrations");
    Arc::new(DBProvider::new(db))
}

/// A backend fake reporting one finding with a configurable category and
/// action, so verdict aggregation can be exercised without the full plugin
/// machinery. (Unlike the real `always-allow-plugin`, which reports an empty
/// finding set — see that gear's module docs.)
struct FakeBackend {
    category: String,
    action: VerdictAction,
}

#[async_trait]
impl ScannerBackendClient for FakeBackend {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Findings(vec![Finding {
            detector: String::new(), // gateway fills this in from the vendor
            category: self.category.clone(),
            severity: Severity::Medium,
            confidence: 0.95,
            span: None,
            suggested_action: self.action,
        }]))
    }
}

/// A backend fake that never responds (used to exercise the per-backend
/// timeout / "unscored" degrade path).
struct SlowBackend;

#[async_trait]
impl ScannerBackendClient for SlowBackend {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        tokio::time::sleep(Duration::from_hours(1)).await;
        unreachable!("timeout should fire first");
    }
}

/// Registers a real (not faked) `file-parser` client in `ClientHub`, backed
/// by the actual `PlainTextParser` backend — mirrors what
/// `FileParserGear::init` does, minus the REST/DB wiring this test doesn't
/// need. Used to prove `Service::resolve_effective_payload` really calls
/// through to `file-parser` rather than just plumbing bytes around.
fn register_file_parser(hub: &ClientHub) {
    let parsers: Vec<Arc<dyn file_parser::domain::parser::FileParserBackend>> =
        vec![Arc::new(PlainTextParser::new())];
    let service = Arc::new(FileParserService::new(
        parsers,
        FileParserServiceConfig {
            max_file_size_bytes: 10 * 1024 * 1024,
            allowed_local_base_dir: std::env::temp_dir(),
        },
    ));
    let client: Arc<dyn FileParserClientV1> = Arc::new(FileParserLocalClient::new(service));
    hub.register::<dyn FileParserClientV1>(client);
}

/// Registers a scanner backend plugin instance in both the mock types-registry and
/// the `ClientHub`, mirroring what a real plugin gear's `init()` does.
fn register_backend(
    hub: &ClientHub,
    registry: &mut Vec<serde_json::Value>,
    vendor: &str,
    client: Arc<dyn ScannerBackendClient>,
) -> String {
    let (instance_id, instance_json) = PluginV1::<ScannerBackendSpecV1>::build_registration(
        &format!("cf.test.{vendor}.detector.v1"),
        vendor,
        100,
    )
    .expect("build_registration");
    registry.push(instance_json);
    hub.register_scoped::<dyn ScannerBackendClient>(ClientScope::gts_id(&instance_id), client);
    instance_id.to_string()
}

/// A writable in-memory `PolicyProvider`, standing in for a provider plugin:
/// prompt-scanner's own fallback is read-only.
#[derive(Default)]
struct InMemoryPolicyProvider(tokio::sync::Mutex<std::collections::HashMap<Uuid, TenantPolicy>>);

#[async_trait]
impl prompt_scanner_sdk::PolicyProvider for InMemoryPolicyProvider {
    async fn get_policy(
        &self,
        _ctx: &SecurityContext,
        tenant_id: Uuid,
    ) -> Result<Option<TenantPolicy>, PromptScannerError> {
        Ok(self.0.lock().await.get(&tenant_id).cloned())
    }

    async fn put_policy(
        &self,
        _ctx: &SecurityContext,
        policy: TenantPolicy,
    ) -> Result<TenantPolicy, PromptScannerError> {
        self.0.lock().await.insert(policy.tenant_id, policy.clone());
        Ok(policy)
    }
}

async fn make_service(hub: Arc<ClientHub>) -> TestService {
    make_service_with_enforcer(hub, mock_enforcer).await
}

/// `make_service`, with the PDP left to the caller — so a test can swap in a
/// resolver with different decision semantics without duplicating the wiring.
async fn make_service_with_enforcer(
    hub: Arc<ClientHub>,
    enforcer: impl Fn() -> PolicyEnforcer,
) -> TestService {
    make_service_over(in_memory_db().await, hub, enforcer)
}

/// `make_service_with_enforcer` over a caller-supplied database, so two
/// services with different PDPs can read and write the same rows.
fn make_service_over(
    db: Arc<DbProvider>,
    hub: Arc<ClientHub>,
    enforcer: impl Fn() -> PolicyEnforcer,
) -> TestService {
    let default_policy_provider: Arc<dyn prompt_scanner_sdk::PolicyProvider> =
        Arc::new(InMemoryPolicyProvider::default());
    Service::new(
        db,
        hub,
        Arc::new(OrmScanRecordRepository::new(toolkit_db::odata::LimitCfg {
            default: 50,
            max: 1000,
        })),
        default_policy_provider,
        enforcer(),
        Duration::from_millis(200),
    )
}

fn scan_request(tenant_id: Uuid) -> ScanRequest {
    ScanRequest {
        tenant_id,
        connector_id: "test-connector".to_owned(),
        content_kind: ContentKind::Prompt,
        payload: "hello world".to_owned(),
        metadata: std::collections::HashMap::default(),
        attachment: None,
    }
}

#[tokio::test]
async fn fan_out_aggregates_block_over_warn() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_backend(
        &hub,
        &mut instances,
        "warn_vendor",
        Arc::new(FakeBackend {
            category: "pii.email".to_owned(),
            action: VerdictAction::Warn,
        }),
    );
    register_backend(
        &hub,
        &mut instances,
        "block_vendor",
        Arc::new(FakeBackend {
            category: "secret.aws_key".to_owned(),
            action: VerdictAction::Block,
        }),
    );

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["warn_vendor".to_owned(), "block_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let verdict = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect("scan should succeed");

    assert_eq!(verdict.action, VerdictAction::Block);
    assert_eq!(verdict.findings.len(), 2);
}

/// A scan of a content kind the policy does not govern runs no backends and
/// is allowed, but is still recorded; a governed kind is scanned as usual.
#[tokio::test]
async fn ungoverned_content_kind_is_allowed_without_scanning() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();
    register_backend(
        &hub,
        &mut instances,
        "block_vendor",
        Arc::new(FakeBackend {
            category: "secret.aws_key".to_owned(),
            action: VerdictAction::Block,
        }),
    );
    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);
    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);
    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["block_vendor".to_owned()];
    policy.content_kinds = vec![ContentKind::Response];
    svc.put_policy(&ctx, policy).await.expect("put_policy");
    assert_eq!(
        svc.get_policy(&ctx)
            .await
            .expect("get_policy")
            .content_kinds,
        vec![ContentKind::Response],
    );

    let prompt = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect("scan should succeed");
    assert_eq!(prompt.action, VerdictAction::Allow);
    assert!(prompt.findings.is_empty());
    let recorded = svc
        .get_verdict(&ctx, prompt.trace_id)
        .await
        .expect("the skipped scan is still recorded");
    assert_eq!(recorded.action, VerdictAction::Allow);

    let mut response = scan_request(tenant_id);
    response.content_kind = ContentKind::Response;
    let response = svc.scan(&ctx, response).await.expect("scan should succeed");
    assert_eq!(response.action, VerdictAction::Block);
}

/// A backend fake that reports every finding it found in one scan — the
/// scenario `BackendOutcome::Findings` exists for: a single message
/// containing more than one distinct piece of sensitive material,
/// detected by a single backend in one `detect` call.
struct MultiFindingBackend {
    findings: Vec<(String, VerdictAction)>,
}

#[async_trait]
impl ScannerBackendClient for MultiFindingBackend {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Findings(
            self.findings
                .iter()
                .map(|(category, action)| Finding {
                    detector: String::new(),
                    category: category.clone(),
                    severity: Severity::Medium,
                    confidence: 0.9,
                    span: None,
                    suggested_action: *action,
                })
                .collect(),
        ))
    }
}

/// A single backend reporting multiple distinct findings from one scan must
/// have *all* of them show up in the aggregated verdict — not just the
/// first, and not collapsed into one.
#[tokio::test]
async fn a_single_backend_can_report_multiple_findings_from_one_scan() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_backend(
        &hub,
        &mut instances,
        "multi_vendor",
        Arc::new(MultiFindingBackend {
            findings: vec![
                ("pii.email".to_owned(), VerdictAction::Redact),
                ("pii.phone".to_owned(), VerdictAction::Redact),
            ],
        }),
    );

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["multi_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let verdict = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect("scan should succeed");

    assert_eq!(verdict.action, VerdictAction::Redact);
    assert_eq!(verdict.findings.len(), 2);
    assert!(verdict.findings.iter().any(|f| f.category == "pii.email"));
    assert!(verdict.findings.iter().any(|f| f.category == "pii.phone"));
}

#[tokio::test]
async fn slow_backend_degrades_to_unscored_when_not_required() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_backend(&hub, &mut instances, "slow_vendor", Arc::new(SlowBackend));
    register_backend(
        &hub,
        &mut instances,
        "fast_vendor",
        Arc::new(FakeBackend {
            category: "pii.email".to_owned(),
            action: VerdictAction::Audit,
        }),
    );

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["slow_vendor".to_owned(), "fast_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let verdict = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect("scan should succeed despite the slow backend timing out");

    // Only the fast backend's finding is recorded; the slow one degraded
    // to "unscored" rather than failing the whole scan (it wasn't marked
    // `required`).
    assert_eq!(verdict.findings.len(), 1);
    assert_eq!(verdict.action, VerdictAction::Audit);
}

#[tokio::test]
async fn required_backend_failure_fails_the_scan() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_backend(&hub, &mut instances, "slow_vendor", Arc::new(SlowBackend));

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["slow_vendor".to_owned()];
    policy.required_backends = vec!["slow_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let err = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect_err("required backend timeout must fail the scan");

    assert!(matches!(
        err,
        crate::domain::error::DomainError::RequiredBackendFailed { .. }
    ));
}

/// An always-permit PDP that returns **no constraints**, i.e. an unconstrained
/// allow. `MockAuthZResolver` always returns a tenant `Eq` constraint, so the
/// unconstrained branch of `get_verdict` was never reached by any test — which
/// is how the `AccessScope::allow_all()` prefetch and the
/// `scope.is_unconstrained()` short-circuit survived: the row a foreign tenant
/// received came from the prefetch, and no fixture could produce that decision.
struct UnconstrainedAllowResolver;

#[async_trait]
impl AuthZResolverApi for UnconstrainedAllowResolver {
    async fn evaluate(
        &self,
        _ctx: PlatformSecurityContext,
        _request: EvaluationRequest,
    ) -> Result<EvaluationResponse, CanonicalError> {
        Ok(EvaluationResponse {
            decision: true,
            context: EvaluationResponseContext {
                constraints: Vec::new(),
                deny_reason: None,
            },
        })
    }
}

/// Regression: a verdict must never be readable across tenants, even when the
/// PDP allows without constraints.
///
/// `get_verdict` prefetches the record to obtain `owner_tenant_id` for the PDP
/// call. That prefetch is scoped to the caller's own tenant, so tenant B's
/// lookup of tenant A's `trace_id` finds nothing and stops before any
/// authorization decision is made. With an `allow_all()` prefetch and a
/// short-circuit on an unconstrained decision, tenant B received tenant A's
/// findings and `redacted_payload` instead.
#[tokio::test]
async fn get_verdict_is_tenant_scoped_even_under_an_unconstrained_allow() {
    let hub = Arc::new(ClientHub::new());
    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(MockTypesRegistryClient::new());
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();

    // Two services over one database. The write path requires constraints, so
    // the record is created under the ordinary mock; the read is then attempted
    // under a PDP that allows without constraining anything.
    let db = in_memory_db().await;
    let writer = make_service_over(Arc::clone(&db), Arc::clone(&hub), mock_enforcer);
    let reader = make_service_over(db, hub, || {
        PolicyEnforcer::new(Arc::new(UnconstrainedAllowResolver))
    });

    let verdict = writer
        .scan(&ctx_for_tenant(tenant_a), scan_request(tenant_a))
        .await
        .expect("scan for tenant A");

    let err = reader
        .get_verdict(&ctx_for_tenant(tenant_b), verdict.trace_id)
        .await
        .expect_err("tenant B must not read tenant A's verdict under an unconstrained allow");
    assert!(
        matches!(
            err,
            crate::domain::error::DomainError::VerdictNotFound { .. }
        ),
        "expected VerdictNotFound, got {err:?}"
    );
}

#[tokio::test]
async fn tenant_isolation_get_verdict() {
    let hub = Arc::new(ClientHub::new());
    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(MockTypesRegistryClient::new());
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let ctx_a = ctx_for_tenant(tenant_a);
    let ctx_b = ctx_for_tenant(tenant_b);

    let verdict = svc
        .scan(&ctx_a, scan_request(tenant_a))
        .await
        .expect("scan for tenant A");

    // Tenant A can read its own verdict back.
    let fetched = svc
        .get_verdict(&ctx_a, verdict.trace_id)
        .await
        .expect("tenant A reads its own verdict");
    assert_eq!(fetched.trace_id, verdict.trace_id);

    // Tenant B cannot read tenant A's verdict.
    let err = svc
        .get_verdict(&ctx_b, verdict.trace_id)
        .await
        .expect_err("tenant B must not see tenant A's verdict");
    assert!(matches!(
        err,
        crate::domain::error::DomainError::VerdictNotFound { .. }
    ));
}

#[tokio::test]
async fn scan_async_completes_via_job_worker() {
    let hub = Arc::new(ClientHub::new());
    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(MockTypesRegistryClient::new());
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;
    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let handle = svc
        .scan_async(&ctx, scan_request(tenant_id))
        .await
        .expect("scan_async queues a job");
    assert_eq!(
        svc.job_status(handle.job_id).await,
        Some(prompt_scanner_sdk::ScanJobStatus::Queued)
    );

    let processed = svc.process_next_job().await;
    assert!(processed);
    assert_eq!(
        svc.job_status(handle.job_id).await,
        Some(prompt_scanner_sdk::ScanJobStatus::Completed)
    );

    let verdict = svc
        .get_verdict(&ctx, handle.trace_id)
        .await
        .expect("verdict recorded under the pre-allocated trace_id");
    assert_eq!(verdict.trace_id, handle.trace_id);
}

/// Backend fake used by
/// `upload_attachment_is_extracted_via_file_parser_before_scanning`: fails
/// the test (via a panic in `detect`) unless the payload it receives is the
/// *decoded plain-text content*, not the raw base64 attachment blob —
/// proving extraction happened upstream of the backend fan-out.
struct AssertExtractedText;

#[async_trait]
impl ScannerBackendClient for AssertExtractedText {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        assert!(
            input.payload.contains("contact me at leaked@example.com"),
            "backend should see file-parser's extracted (Markdown-rendered) text, \
             not the raw base64 attachment: got {:?}",
            input.payload
        );
        Ok(BackendOutcome::Findings(vec![Finding {
            detector: String::new(),
            category: "pii.email".to_owned(),
            severity: Severity::Medium,
            confidence: 0.95,
            span: None,
            suggested_action: VerdictAction::Warn,
        }]))
    }
}

/// End-to-end proof that `prompt-scanner` extracts attachment text itself via
/// `file-parser`, rather than requiring the connector to pre-extract it: see
/// `AssertExtractedText` above for how the extraction is actually verified.
#[tokio::test]
async fn upload_attachment_is_extracted_via_file_parser_before_scanning() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_file_parser(&hub);

    register_backend(
        &hub,
        &mut instances,
        "text_assertion_vendor",
        Arc::new(AssertExtractedText),
    );

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["text_assertion_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let req = ScanRequest {
        tenant_id,
        connector_id: "test-connector".to_owned(),
        content_kind: ContentKind::Upload,
        payload: String::new(),
        metadata: std::collections::HashMap::default(),
        attachment: Some(Attachment {
            filename: Some("note.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            data_base64: base64::engine::general_purpose::STANDARD
                .encode(b"contact me at leaked@example.com"),
        }),
    };

    let verdict = svc
        .scan(&ctx, req)
        .await
        .expect("scan of an upload attachment should succeed");

    assert_eq!(verdict.action, VerdictAction::Warn);
    assert_eq!(verdict.findings.len(), 1);
    assert_eq!(verdict.findings[0].category, "pii.email");
}

/// Invalid base64 in `attachment.data_base64` must fail the scan with
/// `AttachmentParsingFailed`, not panic or silently pass an empty payload to
/// backends.
#[tokio::test]
async fn upload_attachment_with_invalid_base64_fails_closed() {
    let hub = Arc::new(ClientHub::new());
    register_file_parser(&hub);

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(MockTypesRegistryClient::new());
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;
    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let req = ScanRequest {
        tenant_id,
        connector_id: "test-connector".to_owned(),
        content_kind: ContentKind::Upload,
        payload: String::new(),
        metadata: std::collections::HashMap::default(),
        attachment: Some(Attachment {
            filename: Some("note.txt".to_owned()),
            mime_type: Some("text/plain".to_owned()),
            data_base64: "not-valid-base64!!!".to_owned(),
        }),
    };

    let err = svc
        .scan(&ctx, req)
        .await
        .expect_err("invalid base64 attachment must fail the scan");

    assert!(matches!(
        err,
        crate::domain::error::DomainError::AttachmentParsingFailed { .. }
    ));
}

/// A backend that always defers — used to exercise the async-deferral path
/// (see `domain::service`'s module docs): its finding never appears in the
/// synchronous verdict, only after the background worker processes the
/// queued `ScanJob::DeferredBackend` entry.
struct DeferringBackend;

#[async_trait]
impl ScannerBackendClient for DeferringBackend {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Defer)
    }
}

/// Deferred-then-resolved variant used only by
/// `deferred_backend_finding_is_merged_into_existing_record`: `detect`
/// returns a real finding rather than `Defer`, standing in for "the
/// background worker calls the backend a second time and it now
/// responds" — `Service` has no way to distinguish a first vs. a retried
/// call, so a second fake with the real-finding behavior is the simplest
/// way to model the worker's actual call.
struct ResolvedDeferredBackend {
    category: String,
    action: VerdictAction,
}

#[async_trait]
impl ScannerBackendClient for ResolvedDeferredBackend {
    async fn detect(
        &self,
        _ctx: &SecurityContext,
        _input: ScanInput,
    ) -> Result<BackendOutcome, PromptScannerError> {
        Ok(BackendOutcome::Findings(vec![Finding {
            detector: String::new(),
            category: self.category.clone(),
            severity: Severity::Medium,
            confidence: 0.95,
            span: None,
            suggested_action: self.action,
        }]))
    }
}

/// End-to-end proof of the async-deferral mechanism: a fast backend's
/// finding lands in the synchronous verdict immediately, a deferring
/// backend's contribution does not — until `process_next_job` drains the
/// queued `ScanJob::DeferredBackend` entry and merges its finding into the
/// existing `scan_record`, re-aggregating the verdict in place.
/// Regression: the deferred merge must authorize against the tenant carried by
/// the queued job, never against a tenant read out of the record.
///
/// `merge_deferred_findings` used to prefetch the `scan_record` under
/// `AccessScope::allow_all()` and feed *that* row's `tenant_id` to the PDP as
/// `owner_tenant_id` — so the value the authorization decision was about came
/// from an unscoped read, and a `trace_id` belonging to another tenant would
/// have been authorized and updated on its own say-so. There is now no unscoped
/// read at all: the tenant comes from the job's `ScanInput`, and the record is
/// both read and written under the PDP-derived scope.
///
/// A PDP constrained to a *different* tenant therefore cannot merge into this
/// record — the scoped read finds nothing and the merge fails rather than
/// silently rewriting a foreign verdict.
#[tokio::test]
async fn deferred_merge_is_scoped_to_the_jobs_tenant() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();
    register_backend(
        &hub,
        &mut instances,
        "fast_vendor",
        Arc::new(FakeBackend {
            category: "pii.email".to_owned(),
            action: VerdictAction::Audit,
        }),
    );
    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_a = Uuid::new_v4();
    let tenant_b = Uuid::new_v4();
    let ctx_a = ctx_for_tenant(tenant_a);

    let mut policy = TenantPolicy::default_for_tenant(tenant_a);
    policy.enabled_backends = vec!["fast_vendor".to_owned()];
    svc.put_policy(&ctx_a, policy).await.expect("put_policy");

    let verdict = svc
        .scan(&ctx_a, scan_request(tenant_a))
        .await
        .expect("scan for tenant A");
    let before = verdict.action;

    // Tenant B attempts the merge against tenant A's trace_id. `mock_enforcer`
    // constrains every decision to the caller's own tenant, so the scoped read
    // finds nothing and the merge is refused.
    let err = svc
        .merge_deferred_findings(
            &ctx_for_tenant(tenant_b),
            tenant_b,
            verdict.trace_id,
            vec![Finding {
                detector: "intruder".to_owned(),
                category: "pii.email".to_owned(),
                severity: Severity::Critical,
                confidence: 0.99,
                span: None,
                suggested_action: VerdictAction::Block,
            }],
        )
        .await
        .expect_err("tenant B must not merge into tenant A's record");
    assert!(
        matches!(
            err,
            crate::domain::error::DomainError::VerdictNotFound { .. }
        ),
        "expected VerdictNotFound, got {err:?}"
    );

    // Tenant A's verdict is untouched.
    let after = svc
        .get_verdict(&ctx_a, verdict.trace_id)
        .await
        .expect("tenant A reads its own verdict");
    assert_eq!(
        after.action, before,
        "a refused merge must not alter the record"
    );
    assert!(
        after.findings.iter().all(|f| f.detector != "intruder"),
        "the refused finding must not appear: {:?}",
        after.findings
    );
}

#[tokio::test]
async fn deferred_backend_finding_is_merged_into_existing_record() {
    let hub = Arc::new(ClientHub::new());
    let mut instances = Vec::new();

    register_backend(
        &hub,
        &mut instances,
        "fast_vendor",
        Arc::new(FakeBackend {
            category: "pii.email".to_owned(),
            action: VerdictAction::Audit,
        }),
    );
    register_backend(
        &hub,
        &mut instances,
        "deferring_vendor",
        Arc::new(DeferringBackend),
    );

    let mock_registry: Arc<dyn TypesRegistryClient> = Arc::new(
        MockTypesRegistryClient::new().with_instances(instances.into_iter().map(|obj| {
            let id = obj["id"].as_str().expect("instance has id").to_owned();
            make_test_instance(&id, obj)
        })),
    );
    hub.register::<dyn TypesRegistryClient>(mock_registry);

    let svc = make_service(hub).await;

    let tenant_id = Uuid::new_v4();
    let ctx = ctx_for_tenant(tenant_id);

    let mut policy = TenantPolicy::default_for_tenant(tenant_id);
    policy.enabled_backends = vec!["fast_vendor".to_owned(), "deferring_vendor".to_owned()];
    svc.put_policy(&ctx, policy).await.expect("put_policy");

    let verdict = svc
        .scan(&ctx, scan_request(tenant_id))
        .await
        .expect("scan should succeed synchronously despite the deferring backend");

    // Only the fast backend's finding is in the synchronous verdict; the
    // deferring backend's contribution hasn't been processed yet.
    assert_eq!(verdict.findings.len(), 1);
    assert_eq!(verdict.action, VerdictAction::Audit);

    // Swap in a fake that actually resolves, standing in for "the deferred
    // backend responds when the worker retries it" (see
    // `ResolvedDeferredBackend`'s docs), then drain the queued job.
    svc.jobs.lock().await.iter_mut().for_each(|job| {
        if let super::ScanJob::DeferredBackend { client, .. } = job {
            *client = Arc::new(ResolvedDeferredBackend {
                category: "pii.person".to_owned(),
                action: VerdictAction::Block,
            });
        }
    });

    let processed = svc.process_next_job().await;
    assert!(processed, "deferred backend job should have been queued");

    let merged = svc
        .get_verdict(&ctx, verdict.trace_id)
        .await
        .expect("verdict should still exist after the merge");

    assert_eq!(merged.findings.len(), 2);
    assert_eq!(merged.action, VerdictAction::Block);
}
