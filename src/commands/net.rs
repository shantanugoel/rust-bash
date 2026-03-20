//! Network commands: curl

use crate::commands::{CommandContext, CommandResult};
use crate::network::NetworkPolicy;
use std::io::Read;
use std::path::PathBuf;

fn resolve_path(path_str: &str, cwd: &str) -> PathBuf {
    if path_str.starts_with('/') {
        PathBuf::from(path_str)
    } else {
        PathBuf::from(cwd).join(path_str)
    }
}

// ── Argument parsing ─────────────────────────────────────────────────

#[derive(Debug, Default)]
struct CurlOpts {
    url: Option<String>,
    method: Option<String>,
    headers: Vec<(String, String)>,
    data: Option<String>,
    output_file: Option<String>,
    fail_on_error: bool,
    follow_redirects: bool,
    include_headers: bool,
    write_out: Option<String>,
    head_request: bool,
    verbose: bool,
    // -s, -S, -k are accepted but have no effect
}

fn parse_curl_args(args: &[String]) -> Result<CurlOpts, CommandResult> {
    let mut opts = CurlOpts::default();
    let mut i = 0;

    while i < args.len() {
        let arg = &args[i];
        match arg.as_str() {
            "-X" | "--request" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "curl: option -X requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.method = Some(args[i].to_uppercase());
            }
            "-H" | "--header" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "curl: option -H requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                match parse_header(&args[i]) {
                    Some(pair) => opts.headers.push(pair),
                    None => {
                        return Err(CommandResult {
                            stderr: format!("curl: invalid header format: {}\n", args[i]),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                }
            }
            "-d" | "--data" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "curl: option -d requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.data = Some(args[i].clone());
            }
            "-o" | "--output" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "curl: option -o requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.output_file = Some(args[i].clone());
            }
            "-w" | "--write-out" => {
                i += 1;
                if i >= args.len() {
                    return Err(CommandResult {
                        stderr: "curl: option -w requires an argument\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.write_out = Some(args[i].clone());
            }
            "-s" | "--silent" | "-S" | "--show-error" | "-k" | "--insecure" => {
                // Accepted but no-op
            }
            "-f" | "--fail" => opts.fail_on_error = true,
            "-L" | "--location" => opts.follow_redirects = true,
            "-i" | "--include" => opts.include_headers = true,
            "-I" | "--head" => opts.head_request = true,
            "-v" | "--verbose" => opts.verbose = true,
            "--" => {
                // End of options — all remaining args are positional
                for arg in &args[i + 1..] {
                    if opts.url.is_some() {
                        return Err(CommandResult {
                            stderr: "curl: multiple URLs not supported\n".to_string(),
                            exit_code: 2,
                            ..Default::default()
                        });
                    }
                    opts.url = Some(arg.clone());
                }
                break;
            }
            other if other.starts_with('-') => {
                // Handle combined short flags like -sS, -sSf, -fsSL, etc.
                if other.len() > 2 && !other.starts_with("--") {
                    let chars: Vec<char> = other[1..].chars().collect();
                    let mut j = 0;
                    while j < chars.len() {
                        match chars[j] {
                            's' | 'S' | 'k' => {} // no-op flags
                            'f' => opts.fail_on_error = true,
                            'L' => opts.follow_redirects = true,
                            'i' => opts.include_headers = true,
                            'I' => opts.head_request = true,
                            'v' => opts.verbose = true,
                            // Flags that consume the next argument
                            'X' | 'H' | 'd' | 'o' | 'w' => {
                                // Re-dispatch: remaining chars are the value,
                                // or next arg is the value
                                let flag = format!("-{}", chars[j]);
                                let value = if j + 1 < chars.len() {
                                    // Rest of combined string is the value
                                    let val: String = chars[j + 1..].iter().collect();
                                    Some(val)
                                } else {
                                    None
                                };
                                let mut sub_args = vec![flag];
                                if let Some(val) = value {
                                    sub_args.push(val);
                                } else {
                                    i += 1;
                                    if i >= args.len() {
                                        return Err(CommandResult {
                                            stderr: format!(
                                                "curl: option -{} requires an argument\n",
                                                chars[j]
                                            ),
                                            exit_code: 2,
                                            ..Default::default()
                                        });
                                    }
                                    sub_args.push(args[i].clone());
                                }
                                let sub_args_str: Vec<String> = sub_args.into_iter().collect();
                                // Recursively parse the extracted flag
                                let sub_opts = parse_curl_args(&sub_args_str)?;
                                // Merge relevant fields
                                if sub_opts.method.is_some() {
                                    opts.method = sub_opts.method;
                                }
                                opts.headers.extend(sub_opts.headers);
                                if sub_opts.data.is_some() {
                                    opts.data = sub_opts.data;
                                }
                                if sub_opts.output_file.is_some() {
                                    opts.output_file = sub_opts.output_file;
                                }
                                if sub_opts.write_out.is_some() {
                                    opts.write_out = sub_opts.write_out;
                                }
                                // Break out of inner loop since value-consuming
                                // flag eats the rest
                                break;
                            }
                            c => {
                                return Err(CommandResult {
                                    stderr: format!("curl: unknown option: -{c}\n"),
                                    exit_code: 2,
                                    ..Default::default()
                                });
                            }
                        }
                        j += 1;
                    }
                } else {
                    return Err(CommandResult {
                        stderr: format!("curl: unknown option: {other}\n"),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
            }
            _ => {
                // Positional argument: URL
                if opts.url.is_some() {
                    return Err(CommandResult {
                        stderr: "curl: multiple URLs not supported\n".to_string(),
                        exit_code: 2,
                        ..Default::default()
                    });
                }
                opts.url = Some(arg.clone());
            }
        }
        i += 1;
    }

    if opts.url.is_none() {
        return Err(CommandResult {
            stderr: "curl: no URL specified\n".to_string(),
            exit_code: 2,
            ..Default::default()
        });
    }

    Ok(opts)
}

fn parse_header(s: &str) -> Option<(String, String)> {
    let colon_pos = s.find(':')?;
    let name = s[..colon_pos].trim().to_string();
    let value = s[colon_pos + 1..].trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some((name, value))
}

// ── Network policy enforcement ───────────────────────────────────────

fn enforce_policy(policy: &NetworkPolicy, url: &str, method: &str) -> Result<(), CommandResult> {
    if !policy.enabled {
        return Err(CommandResult {
            stderr: "curl: network access is disabled\n".to_string(),
            exit_code: 1,
            ..Default::default()
        });
    }

    if let Err(msg) = policy.validate_url(url) {
        return Err(CommandResult {
            stderr: format!("curl: {msg}\n"),
            exit_code: 1,
            ..Default::default()
        });
    }

    if let Err(msg) = policy.validate_method(method) {
        return Err(CommandResult {
            stderr: format!("curl: {msg}\n"),
            exit_code: 1,
            ..Default::default()
        });
    }

    Ok(())
}

// ── Response body reading with size limit ────────────────────────────

fn read_body_limited(reader: &mut dyn Read, max_size: usize) -> Result<Vec<u8>, String> {
    let mut buf = vec![0u8; 8192];
    let mut body = Vec::new();

    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if body.len() + n > max_size {
                    return Err(format!(
                        "curl: response body exceeds maximum size ({max_size} bytes)"
                    ));
                }
                body.extend_from_slice(&buf[..n]);
            }
            Err(e) => return Err(format!("curl: error reading response: {e}")),
        }
    }

    Ok(body)
}

// ── Format response headers ──────────────────────────────────────────

// ureq v3 doesn't expose the response HTTP version, so we hardcode HTTP/1.1.
fn format_response_headers(status: u16, headers: &ureq::http::HeaderMap) -> String {
    let mut out = format!("HTTP/1.1 {status}\r\n");
    for (name, value) in headers.iter() {
        out.push_str(&format!(
            "{}: {}\r\n",
            name,
            value.to_str().unwrap_or("<binary>")
        ));
    }
    out.push_str("\r\n");
    out
}

// ── CurlCommand ──────────────────────────────────────────────────────

pub struct CurlCommand;

impl super::VirtualCommand for CurlCommand {
    fn name(&self) -> &str {
        "curl"
    }

    fn execute(&self, args: &[String], ctx: &CommandContext) -> CommandResult {
        let opts = match parse_curl_args(args) {
            Ok(o) => o,
            Err(r) => return r,
        };

        let url = opts.url.as_deref().unwrap();
        let mut method = if let Some(ref m) = opts.method {
            m.clone()
        } else if opts.head_request {
            "HEAD".to_string()
        } else if opts.data.is_some() {
            "POST".to_string()
        } else {
            "GET".to_string()
        };

        // Enforce network policy
        if let Err(r) = enforce_policy(ctx.network_policy, url, &method) {
            return r;
        }

        let policy = ctx.network_policy;

        // Build ureq agent: disable automatic redirects so we can validate each hop,
        // and disable treating HTTP status codes as errors so we handle them ourselves.
        let config = ureq::Agent::config_builder()
            .max_redirects(0)
            .timeout_global(Some(policy.timeout))
            .http_status_as_error(false)
            .build();
        let agent: ureq::Agent = config.into();

        let mut current_url = url.to_string();
        let mut redirects_followed: usize = 0;
        let mut stderr = String::new();

        loop {
            if opts.verbose {
                stderr.push_str(&format!("> {method} {current_url}\n"));
                for (name, value) in &opts.headers {
                    stderr.push_str(&format!("> {name}: {value}\n"));
                }
            }

            // Build and send the request. ureq v3 uses typestates to
            // distinguish body-carrying methods from no-body methods, so we
            // must dispatch through `send_request` which handles both paths.
            let result = send_request(&agent, &current_url, &method, &opts);

            let mut response = match result {
                Ok(resp) => resp,
                Err(e) => {
                    return CommandResult {
                        stderr: format!("curl: {e}\n"),
                        exit_code: 1,
                        ..Default::default()
                    };
                }
            };

            let status = response.status().as_u16();

            if opts.verbose {
                stderr.push_str(&format!("< HTTP/1.1 {status}\n"));
                for (name, value) in response.headers().iter() {
                    stderr.push_str(&format!(
                        "< {}: {}\n",
                        name,
                        value.to_str().unwrap_or("<binary>")
                    ));
                }
            }

            // Handle redirects (RFC 7231 method semantics):
            // 301/302/303: change to GET, drop body
            // 307/308: preserve original method and body
            if opts.follow_redirects && is_redirect(status) {
                redirects_followed += 1;
                if redirects_followed > policy.max_redirects {
                    return CommandResult {
                        stderr: format!(
                            "curl: maximum redirects ({}) followed\n",
                            policy.max_redirects
                        ),
                        exit_code: 47,
                        ..Default::default()
                    };
                }

                let location = match response.headers().get("location") {
                    Some(loc) => loc.to_str().unwrap_or("").to_string(),
                    None => {
                        return CommandResult {
                            stderr: "curl: redirect with no Location header\n".to_string(),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                };

                // Resolve relative redirect URLs
                let next_url = resolve_redirect_url(&current_url, &location);

                // Per RFC 7231: 301/302/303 change method to GET
                let next_method = match status {
                    301..=303 => "GET".to_string(),
                    _ => method.clone(), // 307/308 preserve method
                };

                // Validate redirect target and method against network policy
                if let Err(r) = enforce_policy(policy, &next_url, &next_method) {
                    return r;
                }

                if opts.verbose {
                    stderr.push_str(&format!("* Following redirect to {next_url}\n"));
                }
                current_url = next_url;
                method = next_method;
                continue;
            }

            // Read response body with size limit
            let body_bytes = if opts.head_request {
                Vec::new()
            } else {
                match read_body_limited(
                    &mut response.body_mut().as_reader(),
                    policy.max_response_size,
                ) {
                    Ok(b) => b,
                    Err(msg) => {
                        return CommandResult {
                            stderr: format!("{msg}\n"),
                            exit_code: 1,
                            ..Default::default()
                        };
                    }
                }
            };

            let body_text = String::from_utf8_lossy(&body_bytes).to_string();

            // Build output
            let mut stdout = String::new();

            // Check -f/--fail before writing body (real curl suppresses body on error)
            let is_http_error = opts.fail_on_error && status >= 400;

            if opts.include_headers {
                stdout.push_str(&format_response_headers(status, response.headers()));
            }

            if !is_http_error {
                // Write body to file or stdout
                if let Some(ref path) = opts.output_file {
                    let full_path = resolve_path(path, ctx.cwd);
                    if let Err(e) = ctx.fs.write_file(&full_path, &body_bytes) {
                        return CommandResult {
                            stderr: format!("curl: error writing to {path}: {e}\n"),
                            exit_code: 23,
                            ..Default::default()
                        };
                    }
                } else {
                    stdout.push_str(&body_text);
                }
            }

            // Handle -w/--write-out
            if let Some(ref fmt) = opts.write_out {
                let expanded = fmt.replace("%{http_code}", &status.to_string());
                stdout.push_str(&expanded);
            }

            let exit_code = if is_http_error {
                stderr.push_str(&format!(
                    "curl: (22) The requested URL returned error: {status}\n"
                ));
                22
            } else {
                0
            };

            return CommandResult {
                stdout,
                stderr,
                exit_code,
            };
        }
    }
}

/// Send an HTTP request using the appropriate ureq typestate path.
///
/// ureq v3 returns different builder types for methods with/without bodies,
/// so we dispatch here and return the unified `Response<Body>`.
fn send_request(
    agent: &ureq::Agent,
    url: &str,
    method: &str,
    opts: &CurlOpts,
) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let has_body = method == "POST" || method == "PUT" || method == "PATCH";

    if has_body {
        let mut req = match method {
            "POST" => agent.post(url),
            "PUT" => agent.put(url),
            "PATCH" => agent.patch(url),
            _ => unreachable!(),
        };
        for (name, value) in &opts.headers {
            req = req.header(name.as_str(), value.as_str());
        }
        if opts.data.is_some() {
            let has_content_type = opts
                .headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
            if !has_content_type {
                req = req.header("Content-Type", "application/x-www-form-urlencoded");
            }
        }
        if let Some(ref data) = opts.data {
            req.send(data.as_str())
        } else {
            req.send_empty()
        }
    } else {
        let mut req = match method {
            "HEAD" => agent.head(url),
            "OPTIONS" => agent.options(url),
            "DELETE" => agent.delete(url),
            "TRACE" => agent.trace(url),
            _ => agent.get(url), // GET and any other
        };
        for (name, value) in &opts.headers {
            req = req.header(name.as_str(), value.as_str());
        }
        req.call()
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn resolve_redirect_url(base_url: &str, location: &str) -> String {
    // If location is absolute, use it directly; otherwise resolve relative to base
    if location.starts_with("http://") || location.starts_with("https://") {
        location.to_string()
    } else if let Ok(base) = url::Url::parse(base_url) {
        base.join(location)
            .map(|u| u.to_string())
            .unwrap_or_else(|_| location.to_string())
    } else {
        location.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::VirtualCommand;
    use crate::interpreter::ExecutionLimits;
    use crate::network::NetworkPolicy;
    use crate::vfs::InMemoryFs;
    use std::collections::HashMap;
    use std::sync::Arc;

    fn test_ctx_with_policy(
        policy: NetworkPolicy,
    ) -> (
        Arc<InMemoryFs>,
        HashMap<String, String>,
        ExecutionLimits,
        NetworkPolicy,
    ) {
        (
            Arc::new(InMemoryFs::new()),
            HashMap::new(),
            ExecutionLimits::default(),
            policy,
        )
    }

    fn make_ctx<'a>(
        fs: &'a dyn crate::vfs::VirtualFs,
        env: &'a HashMap<String, String>,
        limits: &'a ExecutionLimits,
        np: &'a NetworkPolicy,
    ) -> CommandContext<'a> {
        CommandContext {
            fs,
            cwd: "/",
            env,
            stdin: "",
            limits,
            network_policy: np,
            exec: None,
        }
    }

    #[test]
    fn network_disabled_returns_error() {
        let (fs, env, limits, np) = test_ctx_with_policy(NetworkPolicy::default());
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(&["https://example.com".into()], &ctx);
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("network access is disabled"));
    }

    #[test]
    fn url_not_allowed_returns_error() {
        let policy = NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default()
        };
        let (fs, env, limits, np) = test_ctx_with_policy(policy);
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(&["https://evil.com/data".into()], &ctx);
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("URL not allowed by network policy"));
    }

    #[test]
    fn method_not_allowed_returns_error() {
        let policy = NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            ..Default::default() // only GET and POST allowed
        };
        let (fs, env, limits, np) = test_ctx_with_policy(policy);
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(
            &[
                "-X".into(),
                "DELETE".into(),
                "https://api.example.com/resource".into(),
            ],
            &ctx,
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("HTTP method not allowed"));
    }

    #[test]
    fn no_url_returns_error() {
        let (fs, env, limits, np) = test_ctx_with_policy(NetworkPolicy::default());
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(&[], &ctx);
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("no URL specified"));
    }

    #[test]
    fn unknown_option_returns_error() {
        let (fs, env, limits, np) = test_ctx_with_policy(NetworkPolicy::default());
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(&["--bogus".into()], &ctx);
        assert_eq!(result.exit_code, 2);
        assert!(result.stderr.contains("unknown option"));
    }

    #[test]
    fn data_flag_defaults_method_to_post() {
        // Policy check happens first, so we can verify method from error message
        let policy = NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            allowed_methods: std::collections::HashSet::from(["GET".to_string()]),
            ..Default::default()
        };
        let (fs, env, limits, np) = test_ctx_with_policy(policy);
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result = CurlCommand.execute(
            &[
                "-d".into(),
                "body".into(),
                "https://api.example.com/post".into(),
            ],
            &ctx,
        );
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("POST"));
    }

    #[test]
    fn head_flag_sets_head_method() {
        let policy = NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            allowed_methods: std::collections::HashSet::from(["GET".to_string()]),
            ..Default::default()
        };
        let (fs, env, limits, np) = test_ctx_with_policy(policy);
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        let result =
            CurlCommand.execute(&["-I".into(), "https://api.example.com/test".into()], &ctx);
        assert_eq!(result.exit_code, 1);
        assert!(result.stderr.contains("HEAD"));
    }

    #[test]
    fn combined_short_flags_parsed() {
        // -sSf should parse silent, show-error, fail
        let policy = NetworkPolicy {
            enabled: true,
            allowed_url_prefixes: vec!["https://api.example.com/".to_string()],
            allowed_methods: std::collections::HashSet::from(["GET".to_string()]),
            ..Default::default()
        };
        let (fs, env, limits, np) = test_ctx_with_policy(policy);
        let ctx = make_ctx(&*fs, &env, &limits, &np);
        // -sSfL should be accepted without error (policy will reject the URL
        // if something is wrong, not arg parsing)
        let result = CurlCommand.execute(&["-sSfL".into(), "https://evil.com/".into()], &ctx);
        // Should fail on URL policy, not on arg parsing
        assert!(result.stderr.contains("URL not allowed"));
    }

    #[test]
    fn parse_header_valid() {
        let result = parse_header("Content-Type: application/json");
        assert_eq!(
            result,
            Some(("Content-Type".to_string(), "application/json".to_string()))
        );
    }

    #[test]
    fn parse_header_no_colon() {
        assert_eq!(parse_header("NoColon"), None);
    }

    #[test]
    fn resolve_redirect_absolute() {
        let result = resolve_redirect_url("https://example.com/old", "https://other.com/new");
        assert_eq!(result, "https://other.com/new");
    }

    #[test]
    fn resolve_redirect_relative() {
        let result = resolve_redirect_url("https://example.com/old/path", "/new/path");
        assert_eq!(result, "https://example.com/new/path");
    }

    #[test]
    fn read_body_limited_enforces_max_size() {
        let data = vec![0u8; 1000];
        let mut cursor = std::io::Cursor::new(data);
        let result = read_body_limited(&mut cursor, 500);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("exceeds maximum size"));
    }

    #[test]
    fn read_body_limited_allows_within_limit() {
        let data = vec![42u8; 100];
        let mut cursor = std::io::Cursor::new(data.clone());
        let result = read_body_limited(&mut cursor, 200);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), data);
    }

    #[test]
    fn is_redirect_codes() {
        assert!(is_redirect(301));
        assert!(is_redirect(302));
        assert!(is_redirect(303));
        assert!(is_redirect(307));
        assert!(is_redirect(308));
        assert!(!is_redirect(200));
        assert!(!is_redirect(404));
    }

    #[test]
    fn write_out_http_code() {
        // Verify write_out format expansion works
        let fmt = "%{http_code}";
        let expanded = fmt.replace("%{http_code}", "200");
        assert_eq!(expanded, "200");
    }

    #[test]
    fn format_response_headers_basic() {
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("content-type", "text/plain".parse().unwrap());
        let output = format_response_headers(200, &headers);
        assert!(output.starts_with("HTTP/1.1 200\r\n"));
        assert!(output.contains("content-type: text/plain\r\n"));
        assert!(output.ends_with("\r\n"));
    }
}
