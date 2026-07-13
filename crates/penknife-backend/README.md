# penknife-backend

[![crates.io](https://img.shields.io/crates/v/penknife-backend.svg)](https://crates.io/crates/penknife-backend)
[![docs.rs](https://img.shields.io/docsrs/penknife-backend)](https://docs.rs/penknife-backend)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/jhheider/penknife/blob/main/LICENSE)

The backend contract that [penknife](https://crates.io/crates/penknife)
publishes and syncs documents through.

A backend is one remote service that can hold a copy of a local document
(GitHub Gists today, via
[`penknife-gist`](https://crates.io/crates/penknife-gist); the seam is here for
more). The trait is deliberately small: it mirrors exactly what penknife's
sync engine consumes, so a single implementation unlocks the whole UI.

## The contract

Every backend is a *sync* backend: content round-trips losslessly, so pulling
remote content back over the local file is always safe.

```rust
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use penknife_backend::{Backend, RemoteChange, RemoteDoc, RemoteRef, Result};

struct MyBackend;

#[async_trait]
impl Backend for MyBackend {
    fn name(&self) -> &'static str { "example" }

    async fn create(&self, filename: &str, content: &str, description: &str) -> Result<RemoteRef> {
        todo!()
    }
    async fn read(&self, remote_id: &str, filename: &str) -> Result<RemoteDoc> {
        todo!()
    }
    async fn update(&self, remote_id: &str, filename: &str, content: &str) -> Result<RemoteRef> {
        todo!()
    }
    async fn delete(&self, remote_id: &str) -> Result<()> {
        todo!()
    }
    async fn changed_since(&self, since: Option<DateTime<Utc>>) -> Result<Vec<RemoteChange>> {
        todo!()
    }
}
```

- **`create` / `read` / `update` / `delete`** are the round-trip. `filename`
  selects a file within container-shaped backends (a gist holds several files);
  single-document backends may ignore it.
- **`changed_since`** is an optional change feed powering cheap polling and
  incremental hydration. Backends without one return
  `BackendError::ChangesUnsupported` and callers fall back to per-document
  reads.

The trait is object-safe by design: penknife holds backends as
`Arc<dyn Backend>` and dispatches by the `backend` field recorded on each
stored copy.

## License

MIT © Jacob Heider. See [LICENSE](https://github.com/jhheider/penknife/blob/main/LICENSE).
