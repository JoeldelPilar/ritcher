# Module: `http_retry`

Source: `src/http_retry.rs`

Provides `RetryPolicy` — the domain type that expresses HTTP retry semantics for Stitcher fetches.

## `RetryPolicy`

```rust
RetryPolicy {
    max_attempts: u32       // default: 2 (1 initial + 1 retry)
    backoff: Duration       // default: 500ms, fixed (not exponential)
    timeout: Option<Duration> // default: None (client-level timeout applies)
}
```

Constructors:

| Constructor | Attempts | Backoff | Timeout |
|-------------|----------|---------|---------|
| `RetryPolicy::production()` | 2 | 500ms | None |
| `RetryPolicy::no_retries()` | 1 | 0ms | None |

Builder methods: `.with_attempts(n)`, `.with_backoff(d)`, `.with_timeout(d)` — all compose onto an existing policy.

## `execute`

```rust
async fn execute(&self, client: &Client, url: &str) -> Result<Response, reqwest::Error>
```

Makes up to `max_attempts` GET requests against `url`. On non-2xx or network error before the final attempt, logs a warning and sleeps `backoff`, then retries. The final attempt's non-2xx status is converted to an error via `error_for_status()`.

The policy borrows the caller's `Client` rather than owning one. Ritcher has a single shared `reqwest::Client` per process (with the SSRF redirect policy and `SsrfSafeResolver`); multiple call sites compose different `RetryPolicy` values against that same client.

## Call sites

| Location | Policy |
|----------|--------|
| `AppState::default_retry_policy` | `RetryPolicy::production()` — used by origin/segment handlers |
| `VastAdProvider` | `RetryPolicy::production().with_timeout(self.timeout)` — composed per provider instance |
| Integration tests | `RetryPolicy::no_retries()` or `production().with_backoff(1ms)` |

## Backoff note

The backoff is **fixed**, not exponential. For a manifest stitcher handling live streams, bounded retry latency matters more than avoiding load on the upstream: a 500ms backoff on 2 attempts caps added latency at 500ms, which is acceptable for a manifest request but would be ruinous for exponential retries in a thundering-herd scenario.
