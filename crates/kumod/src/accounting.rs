//! The purpose of this module is to keep an overall accounting
//! of the volume of messages that were received and delivered
//! by this instance

use anyhow::Context;
use chrono::prelude::*;
use core::sync::atomic::AtomicUsize;
use kumo_server_lifecycle::ShutdownSubcription;
use kumo_server_runtime::get_main_runtime;
use parking_lot::FairMutex as Mutex;
use sqlite::{Connection, ConnectionThreadSafe};
use std::sync::atomic::Ordering;
use std::sync::LazyLock;
use tokio::task::JoinHandle;

pub static ACCT: LazyLock<Accounting> = LazyLock::new(Accounting::default);
static FLUSHER: LazyLock<Mutex<Option<JoinHandle<()>>>> =
    LazyLock::new(|| Mutex::new(Some(get_main_runtime().spawn(flusher()))));
pub static DB_PATH: LazyLock<Mutex<String>> =
    LazyLock::new(|| Mutex::new("/var/spool/kumomta/accounting.db".to_string()));

#[derive(Default)]
pub struct Accounting {
    received: AtomicUsize,
    delivered: AtomicUsize,
}

impl Accounting {
    /// Increment the received counter by the specified amount
    fn inc_received(&self, amount: usize) {
        self.received.fetch_add(amount, Ordering::SeqCst);
        // and ensure that the flusher gets started
        LazyLock::force(&FLUSHER);
    }

    /// Increment the delivered counter by the specified amount
    fn inc_delivered(&self, amount: usize) {
        self.delivered.fetch_add(amount, Ordering::SeqCst);
        // and ensure that the flusher gets started
        LazyLock::force(&FLUSHER);
    }

    /// Grab the current counters, zeroing the state out.
    fn grab(&self) -> (usize, usize) {
        let mut received;
        loop {
            received = self.received.load(Ordering::SeqCst);
            if self
                .received
                .compare_exchange(received, 0, Ordering::SeqCst, Ordering::SeqCst)
                == Ok(received)
            {
                break;
            }
        }
        let mut delivered;
        loop {
            delivered = self.delivered.load(Ordering::SeqCst);
            if self
                .delivered
                .compare_exchange(delivered, 0, Ordering::SeqCst, Ordering::SeqCst)
                == Ok(delivered)
            {
                break;
            }
        }
        (received, delivered)
    }

    pub async fn wait_for_shutdown(&self) -> anyhow::Result<()> {
        if ShutdownSubcription::try_get().is_none() {
            tracing::trace!("wait_for_shutdown: didn't properly start, ignoring");
            return Ok(());
        }
        if config::is_validating() {
            tracing::trace!("wait_for_shutdown: is_validating, ignoring");
            return Ok(());
        }

        if let Some(handle) = FLUSHER.lock().take() {
            handle.await.ok();
        }

        tracing::trace!("doing final accounting flush");
        self.flush(true).await
    }

    async fn flush(&self, force: bool) -> anyhow::Result<()> {
        if config::is_validating() {
            return Ok(());
        }

        let (received, delivered) = self.grab();

        if !force && (received + delivered == 0) {
            // Nothing to do
            return Ok(());
        }

        let res = get_main_runtime()
            .spawn_blocking(move || {
                tracing::trace!("flushing");
                let db = open_accounting_db().context("open_accounting_db")?;

                let now = Utc::now().date_naive();
                let now = now.format("%Y-%m-01 00:00:00").to_string();

                let mut insert = db
                    .prepare(
                        "INSERT INTO accounting
                    (event_time, received, delivered)
                    values ($now, $received, $delivered)
                    on conflict (event_time)
                    do update set received=received+$received, delivered=delivered+$delivered
                ",
                    )
                    .context("prepare")?;

                insert.bind(("$now", now.as_str())).context("bind $now")?;
                insert
                    .bind(("$received", received as i64))
                    .context("bind $received")?;
                insert
                    .bind(("$delivered", delivered as i64))
                    .context("bind $delivered")?;

                insert.next()?;
                tracing::trace!("flushed");
                Ok(())
            })
            .await?;

        tracing::trace!("flush result is {res:?}");

        if res.is_err() {
            self.inc_received(received);
            self.inc_delivered(delivered);

            tracing::error!(
                "Failed to record {received} receptions + \
                {delivered} deliveries to accounting log, will retry later"
            );
        }

        res
    }
}

/// Only record protocols that correspond to ingress/egress.
/// At this time, that means everything except LogRecords produced
/// by logging hooks
fn is_accounted_protocol(protocol: &str) -> bool {
    protocol != "LogRecord"
}

/// Called by the logging layer to account for a reception
pub fn account_reception(protocol: &str) {
    if !is_accounted_protocol(protocol) {
        return;
    }
    ACCT.inc_received(1);
}

/// Called by the logging layer to account for a delivery
pub fn account_delivery(protocol: &str) {
    if !is_accounted_protocol(protocol) {
        return;
    }
    ACCT.inc_delivered(1);
}

fn open_accounting_db() -> anyhow::Result<ConnectionThreadSafe> {
    let path = DB_PATH.lock().clone();
    tracing::trace!("using path {path:?} for accounting db");
    let mut db = Connection::open_thread_safe(&path)
        .with_context(|| format!("opening accounting database {path}"))?;
    db.set_busy_timeout(30_000)?;

    let query = r#"
CREATE TABLE IF NOT EXISTS accounting (
    event_time DATETIME NOT NULL PRIMARY KEY,
    received int NOT NULL,
    delivered int NOT NULL
);
    "#;

    db.execute(query)?;

    tracing::trace!("completed setup for {path:?}");

    Ok(db)
}

async fn flusher() {
    tracing::trace!("flusher started");
    let mut shutdown = ShutdownSubcription::get();
    loop {
        tokio::select! {
            _ = shutdown.shutting_down() => {
                tracing::trace!("flusher shutting down");
                break;
            },
            _ = tokio::time::sleep(std::time::Duration::from_secs(5 * 60)) => {}
        };

        let result = ACCT.flush(false).await;
        if let Err(err) = result {
            tracing::error!("Error flushing accounting logs: {err:#}");
        }
    }
}
