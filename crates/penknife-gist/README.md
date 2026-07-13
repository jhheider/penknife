# penknife-gist

[![crates.io](https://img.shields.io/crates/v/penknife-gist.svg)](https://crates.io/crates/penknife-gist)
[![docs.rs](https://img.shields.io/docsrs/penknife-gist)](https://docs.rs/penknife-gist)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](https://github.com/jhheider/penknife/blob/main/LICENSE)

An async GitHub Gist API client, with retry, rate-limit, and pagination
handling built in. It is the GitHub backend for
[penknife](https://crates.io/crates/penknife), split out so it can be used on
its own.

## What it does

- **Full CRUD over gists** — list, get, create, update, delete, and rename a
  file within a gist.
- **Retries transient failures** — network errors and `5xx` responses are
  retried with exponential backoff (up to 3 attempts).
- **Respects rate limits** — honors `Retry-After` and
  `X-RateLimit-Remaining`/`X-RateLimit-Reset`, sleeping until the window
  resets rather than hammering the API.
- **Paginates** — walk one page at a time (`list_page`, `list_page_since`) or
  fetch everything (`list_all`); a `since` cursor supports cheap incremental
  polling.
- **Fetches file content** honestly — small files come from the `raw_url`;
  files past GitHub's 10 MB raw-fetch limit surface a `TooLarge` error instead
  of silently truncating.
- **Implements [`penknife-backend`](https://crates.io/crates/penknife-backend)'s
  `Backend` trait**, so it drops into penknife's sync engine as an
  `Arc<dyn Backend>`.
- **rustls + ring** for TLS — no OpenSSL, no aws-lc, no C toolchain in the
  build.

## Usage

```rust
use penknife_gist::GistClient;

# async fn run() -> Result<(), Box<dyn std::error::Error>> {
let client = GistClient::new(std::env::var("GITHUB_TOKEN")?);

// Create a gist.
let gist = client
    .create("notes.md", "# Hello\n", "shared from my machine")
    .await?;
println!("{}", gist.html_url);

// Read a file back out of it.
if let Some(body) = client.file_content(&gist, "notes.md").await? {
    println!("{body}");
}
# Ok(())
# }
```

Point the client at GitHub Enterprise or a mock server with
`GistClient::with_base_url`.

### Resolving a token

`penknife_gist::auth::resolve_token` reads `$GITHUB_TOKEN`, falling back to
`gh auth token` from the [GitHub CLI](https://cli.github.com):

```rust
let token = penknife_gist::auth::resolve_token()?;
let client = penknife_gist::GistClient::new(token);
```

The token needs the `gist` scope. If you authenticated with `gh` before you
cared about gists, add it with `gh auth refresh -s gist`.

## License

MIT © Jacob Heider. See [LICENSE](https://github.com/jhheider/penknife/blob/main/LICENSE).
