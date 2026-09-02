//! Shared harness for the integration tests.
//!
//! Each test gets an isolated database from `#[sqlx::test]`. This spins up the
//! real invoice-service router and the real mock PSP, both in-process on
//! ephemeral ports, and drives them with `reqwest`.

#![allow(dead_code)]

use std::time::Duration;

use invoice_service::{app, auth, config::Config};
use sqlx::PgPool;
use uuid::Uuid;

/// Knobs the failure-mode tests need to turn down so they run in seconds.
pub struct Timings {
    pub psp_fast_delay: Duration,
    pub psp_timeout_delay: Duration,
    pub client_psp_timeout: Duration,
    pub sweep_interval: Duration,
    pub pending_max_age: Duration,
}

impl Default for Timings {
    fn default() -> Self {
        Self {
            psp_fast_delay: Duration::from_millis(20),
            psp_timeout_delay: Duration::from_secs(30),
            client_psp_timeout: Duration::from_secs(5),
            sweep_interval: Duration::from_millis(200),
            pending_max_age: Duration::from_secs(300),
        }
    }
}

pub struct TestApp {
    pub base: String,
    pub psp_debug: String,
    pub pool: PgPool,
    pub api_key: String,
    pub business_id: Uuid,
    pub client: reqwest::Client,
}

pub async fn spawn(pool: PgPool) -> TestApp {
    spawn_with(pool, Timings::default()).await
}

pub async fn spawn_with(pool: PgPool, t: Timings) -> TestApp {
    // Migrations were already applied by `#[sqlx::test]`.
    let (business_id, api_key) = auth::seed(&pool).await.expect("seed a business + key");

    // Mock PSP on an ephemeral port.
    let psp_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let psp_addr = psp_listener.local_addr().unwrap();
    let psp_router = mock_psp::router(mock_psp::MockConfig {
        fast_delay: t.psp_fast_delay,
        timeout_delay: t.psp_timeout_delay,
    });
    tokio::spawn(async move {
        axum::serve(psp_listener, psp_router).await.unwrap();
    });

    // The service. The pool comes straight from `#[sqlx::test]`; `database_url`
    // and `bind_addr` are unused here.
    let config = Config {
        database_url: String::new(),
        psp_base_url: format!("http://{psp_addr}"),
        psp_timeout: t.client_psp_timeout,
        bind_addr: String::new(),
        webhook_worker_interval: Duration::from_millis(200),
        webhook_lease: Duration::from_secs(30),
        payment_sweep_interval: t.sweep_interval,
        payment_pending_max_age: t.pending_max_age,
        webhook_allow_private_targets: true,
    };
    let state = app::build_state(pool.clone(), config);
    invoice_service::sweeper::spawn(state.clone());

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = app::router(state);
    tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });

    TestApp {
        base: format!("http://{addr}"),
        psp_debug: format!("http://{psp_addr}"),
        pool,
        api_key,
        business_id,
        client: reqwest::Client::new(),
    }
}

impl TestApp {
    fn bearer(&self) -> String {
        format!("Bearer {}", self.api_key)
    }

    pub async fn create_customer(&self) -> Uuid {
        let r = self
            .client
            .post(format!("{}/v1/customers", self.base))
            .header("authorization", self.bearer())
            .json(&serde_json::json!({ "name": "T", "email": "t@example.com" }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "create_customer");
        id_of(r).await
    }

    pub async fn create_invoice(&self, cents: i64) -> Uuid {
        let customer_id = self.create_customer().await;
        let r = self
            .client
            .post(format!("{}/v1/invoices", self.base))
            .header("authorization", self.bearer())
            .json(&serde_json::json!({
                "customer_id": customer_id,
                "due_date": "2026-06-01",
                "line_items": [{ "description": "x", "quantity": 1, "unit_amount_cents": cents }],
            }))
            .send()
            .await
            .unwrap();
        assert_eq!(r.status(), 201, "create_invoice");
        id_of(r).await
    }

    pub async fn pay(&self, invoice: Uuid, token: &str, key: &str) -> reqwest::Response {
        self.client
            .post(format!("{}/v1/invoices/{invoice}/pay", self.base))
            .header("authorization", self.bearer())
            .header("idempotency-key", key)
            .json(&serde_json::json!({ "card_token": token }))
            .send()
            .await
            .unwrap()
    }

    pub async fn invoice_state(&self, invoice: Uuid) -> String {
        let r = self
            .client
            .get(format!("{}/v1/invoices/{invoice}", self.base))
            .header("authorization", self.bearer())
            .send()
            .await
            .unwrap();
        r.json::<serde_json::Value>().await.unwrap()["state"]
            .as_str()
            .unwrap()
            .to_owned()
    }

    /// Number of completed charges the mock has recorded.
    pub async fn charge_count(&self) -> usize {
        let r = self
            .client
            .get(format!("{}/_debug/charges", self.psp_debug))
            .send()
            .await
            .unwrap();
        r.json::<Vec<serde_json::Value>>().await.unwrap().len()
    }

    pub async fn succeeded_attempts(&self, invoice: Uuid) -> i64 {
        sqlx::query_scalar(
            "SELECT count(*) FROM payment_attempts WHERE invoice_id = $1 AND status = 'succeeded'",
        )
        .bind(invoice)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }

    pub async fn attempt_status(&self, invoice: Uuid) -> String {
        sqlx::query_scalar(
            "SELECT status FROM payment_attempts WHERE invoice_id = $1 ORDER BY created_at LIMIT 1",
        )
        .bind(invoice)
        .fetch_one(&self.pool)
        .await
        .unwrap()
    }
}

async fn id_of(r: reqwest::Response) -> Uuid {
    r.json::<serde_json::Value>().await.unwrap()["id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap()
}
