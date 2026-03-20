# Text Processing Pipelines

## Goal

Build data-processing pipelines using the 80+ built-in commands. rust-bash supports the standard Unix text-processing toolkit — grep, sed, awk, jq, sort, cut, and more — all running in-process without shelling out to the host.

## Grep, Sort, Uniq — Filtering and Counting

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/access.log".into(), b"\
GET /api/users 200
POST /api/users 201
GET /api/items 200
GET /api/users 200
GET /api/items 404
POST /api/items 201
GET /api/users 200
".to_vec()),
    ]))
    .build()
    .unwrap();

// Count requests per endpoint
let result = shell.exec(
    "grep 'GET' /access.log | cut -d' ' -f2 | sort | uniq -c | sort -rn"
).unwrap();
// Output: most-requested GET endpoints with counts
assert!(result.stdout.contains("/api/users"));
```

## Sed — Stream Editing

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/config.ini".into(), b"\
[database]
host=localhost
port=5432
name=mydb_dev
".to_vec()),
    ]))
    .build()
    .unwrap();

// Replace dev database with production
let result = shell.exec(
    "sed 's/localhost/db.prod.internal/; s/mydb_dev/mydb_prod/' /config.ini"
).unwrap();
assert!(result.stdout.contains("db.prod.internal"));
assert!(result.stdout.contains("mydb_prod"));

// In-place editing with -i
shell.exec("sed -i 's/5432/5433/' /config.ini").unwrap();
let result = shell.exec("cat /config.ini").unwrap();
assert!(result.stdout.contains("5433"));
```

### Sed features supported

- Substitution: `s/pattern/replacement/flags` (g, p, i, nth occurrence)
- Delete: `d`, Print: `p`, Quit: `q`
- Append/Insert/Change: `a`, `i`, `c`
- Address types: line numbers, ranges (`2,5`), regex (`/pattern/`), step (`1~2`)
- Hold buffer: `g`, `G`, `h`, `H`, `x`
- Branching: `b`, `t`, `:label`
- Extended regex with `-E`

## Awk — Field Processing

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data.csv".into(), b"\
name,department,salary
Alice,Engineering,95000
Bob,Marketing,72000
Carol,Engineering,98000
Dave,Marketing,68000
Eve,Engineering,105000
".to_vec()),
    ]))
    .build()
    .unwrap();

// Average salary by department
let result = shell.exec(
    r#"awk -F, 'NR>1 { sum[$2]+=$3; count[$2]++ } END { for (d in sum) print d, sum[d]/count[d] }' /data.csv"#
).unwrap();
assert!(result.stdout.contains("Engineering"));
assert!(result.stdout.contains("Marketing"));

// Filter rows and reformat
let result = shell.exec(
    r#"awk -F, 'NR>1 && $3 > 90000 { printf "%s earns $%d\n", $1, $3 }' /data.csv"#
).unwrap();
assert!(result.stdout.contains("Alice"));
assert!(result.stdout.contains("Eve"));
```

### Awk features supported

- Field splitting (`-F`), variables (`-v`), program files (`-f`)
- Built-in variables: `NR`, `NF`, `FS`, `RS`, `OFS`, `ORS`
- Patterns: `BEGIN`, `END`, regex, expressions
- Control flow: `if`/`else`, `for`, `while`, `do-while`
- Functions: `print`, `printf`, `length`, `substr`, `index`, `split`, `sub`, `gsub`, `match`, `tolower`, `toupper`
- Associative arrays and user-defined functions

## Jq — JSON Processing

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/users.json".into(), br#"[
  {"name": "Alice", "role": "admin", "active": true},
  {"name": "Bob", "role": "user", "active": false},
  {"name": "Carol", "role": "admin", "active": true}
]"#.to_vec()),
    ]))
    .build()
    .unwrap();

// Extract active admin names
let result = shell.exec(
    r#"jq -r '.[] | select(.role == "admin" and .active) | .name' /users.json"#
).unwrap();
assert_eq!(result.stdout, "Alice\nCarol\n");

// Transform structure
let result = shell.exec(
    r#"jq '[.[] | {user: .name, is_admin: (.role == "admin")}]' /users.json"#
).unwrap();
assert!(result.stdout.contains("is_admin"));
```

### Jq features supported

- Full jq filter syntax (powered by jaq)
- Flags: `-r` (raw), `-c` (compact), `-s` (slurp), `-e` (exit status), `-n` (null input), `-S` (sort keys)
- `--arg NAME VAL` and `--argjson NAME JSON` for passing external values

## Diff — Comparing Files

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/v1.txt".into(), b"line1\nline2\nline3\n".to_vec()),
        ("/v2.txt".into(), b"line1\nmodified\nline3\nnew line\n".to_vec()),
    ]))
    .build()
    .unwrap();

// Unified diff format
let result = shell.exec("diff -u /v1.txt /v2.txt").unwrap();
assert!(result.stdout.contains("-line2"));
assert!(result.stdout.contains("+modified"));
```

## Chaining It All Together

A realistic data processing pipeline:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/events.jsonl".into(), b"\
{\"ts\":\"2024-01-15\",\"type\":\"login\",\"user\":\"alice\"}
{\"ts\":\"2024-01-15\",\"type\":\"purchase\",\"user\":\"bob\"}
{\"ts\":\"2024-01-15\",\"type\":\"login\",\"user\":\"alice\"}
{\"ts\":\"2024-01-15\",\"type\":\"login\",\"user\":\"carol\"}
{\"ts\":\"2024-01-15\",\"type\":\"purchase\",\"user\":\"alice\"}
".to_vec()),
    ]))
    .build()
    .unwrap();

// Count login events per user
let result = shell.exec(r#"
    cat /events.jsonl \
      | jq -r 'select(.type == "login") | .user' \
      | sort | uniq -c | sort -rn
"#).unwrap();
assert!(result.stdout.contains("alice"));

// Summarize event types
let result = shell.exec(r#"
    cat /events.jsonl \
      | jq -r '.type' \
      | sort | uniq -c | sort -rn
"#).unwrap();
assert!(result.stdout.contains("login"));
assert!(result.stdout.contains("purchase"));
```

## Find + Xargs — Batch Operations

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/src/main.rs".into(), b"fn main() { todo!() }".to_vec()),
        ("/src/lib.rs".into(), b"pub fn hello() { todo!() }".to_vec()),
        ("/src/utils.rs".into(), b"pub fn util() {}".to_vec()),
    ]))
    .build()
    .unwrap();

// Find files containing "todo" using find -exec
let result = shell.exec("find /src -name '*.rs' -exec grep -l 'todo' {} +").unwrap();
assert!(result.stdout.contains("main.rs"));
assert!(result.stdout.contains("lib.rs"));
assert!(!result.stdout.contains("utils.rs"));
```

## Quick Reference

| Task | Command |
|------|---------|
| Search text | `grep -i pattern file` |
| Search recursively | `grep -r pattern /dir` |
| Replace text | `sed 's/old/new/g' file` |
| Extract columns | `cut -d',' -f1,3 file` |
| Sort lines | `sort -n file` |
| Deduplicate | `sort file \| uniq` |
| Count lines/words | `wc -l file` |
| First/last N lines | `head -n 5 file` / `tail -n 5 file` |
| Reverse lines | `tac file` |
| JSON query | `jq '.key' file.json` |
| Field processing | `awk '{print $1}' file` |
| Character translation | `tr '[:lower:]' '[:upper:]'` |
| Number lines | `nl file` or `cat -n file` |
| Wrap long lines | `fold -w 80 file` |
| Format columns | `column -t file` |
| Compare files | `diff -u file1 file2` |
