# Shell Scripting Features

## Goal

Leverage rust-bash's full bash syntax support: variables, control flow, functions, subshells, arithmetic, and more.

## Variables and Expansion

### Basic variables

```bash
NAME="world"
echo "Hello, $NAME"          # Hello, world
echo "Hello, ${NAME}!"       # Hello, world!
```

### Parameter expansion operators

```bash
# Default values
echo ${UNSET:-default}        # default (UNSET stays unset)
echo ${UNSET:=fallback}       # fallback (UNSET is now "fallback")

# Error if unset
echo ${REQUIRED:?must be set} # error if REQUIRED is unset

# Alternative value
VAR=hello
echo ${VAR:+exists}           # exists (because VAR is set)

# String length
echo ${#VAR}                  # 5

# Substring
echo ${VAR:1:3}               # ell

# Case modification
echo ${VAR^}                  # Hello (capitalize first)
echo ${VAR^^}                 # HELLO (capitalize all)
STR=HELLO
echo ${STR,}                  # hELLO (lowercase first)
echo ${STR,,}                 # hello (lowercase all)
```

### Pattern removal and substitution

```bash
FILE="archive.tar.gz"
echo ${FILE%.*}               # archive.tar  (remove shortest suffix)
echo ${FILE%%.*}              # archive      (remove longest suffix)
echo ${FILE#*.}               # tar.gz       (remove shortest prefix)
echo ${FILE##*.}              # gz           (remove longest prefix)

echo ${FILE/tar/zip}          # archive.zip.gz (replace first match)
echo ${FILE//a/A}             # Archive.tAr.gz (replace all)
```

### Special variables

```bash
echo $?                       # Exit code of last command
echo $$                       # Shell PID (always 1 in rust-bash)
echo $0                       # Shell name ("rust-bash")
set -- a b c
echo $#                       # 3 (positional param count)
echo $1 $2 $3                 # a b c
echo $@                       # a b c (all params)
echo $RANDOM                  # Random number 0–32767
```

## Control Flow

### If/elif/else

```bash
if [ -f /data.txt ]; then
    echo "file exists"
elif [ -d /data ]; then
    echo "directory exists"
else
    echo "nothing found"
fi
```

### Test expressions

```bash
# File tests
[ -e /path ]     # exists
[ -f /path ]     # is regular file
[ -d /path ]     # is directory
[ -L /path ]     # is symlink
[ -s /path ]     # exists and non-empty

# String tests
[ -z "$VAR" ]    # is empty
[ -n "$VAR" ]    # is non-empty
[ "$A" = "$B" ]  # string equality
[ "$A" != "$B" ] # string inequality

# Numeric comparisons
[ "$X" -eq 5 ]   # equal
[ "$X" -lt 10 ]  # less than
[ "$X" -ge 0 ]   # greater or equal
```

### For loops

```bash
# Iterate over a list
for item in apple banana cherry; do
    echo "Fruit: $item"
done

# Iterate over command output
for file in $(find /src -name '*.rs'); do
    echo "Found: $file"
done

# C-style for loop
for ((i=0; i<5; i++)); do
    echo "i=$i"
done
```

### While and until

```bash
# While loop
count=0
while [ $count -lt 5 ]; do
    echo "count=$count"
    count=$((count + 1))
done

# Until loop (runs until condition is true)
n=10
until [ $n -le 0 ]; do
    echo "$n"
    n=$((n - 2))
done
```

### Case statements

```bash
STATUS="error"
case $STATUS in
    ok|success)
        echo "All good"
        ;;
    error)
        echo "Something failed"
        ;;
    *)
        echo "Unknown status: $STATUS"
        ;;
esac
```

## Functions

```bash
# Define a function
greet() {
    local name="${1:-world}"
    echo "Hello, $name!"
}

# Call it
greet Alice    # Hello, Alice!
greet          # Hello, world!

# Functions can use local variables
counter() {
    local count=0
    for item in "$@"; do
        count=$((count + 1))
    done
    echo "$count"
}

counter a b c  # 3

# Return values (exit codes)
is_even() {
    [ $(($1 % 2)) -eq 0 ]
}

if is_even 4; then
    echo "4 is even"
fi
```

## Arithmetic

### $(( )) expressions

```bash
echo $((2 + 3))           # 5
echo $((10 / 3))          # 3
echo $((2 ** 10))         # 1024
echo $((x = 5, x * 2))   # 10

# All C-style operators
echo $((a=10, b=3, a % b))     # 1
echo $((1 << 8))               # 256
echo $((0xFF & 0x0F))          # 15
echo $((a > b ? a : b))        # 10 (ternary)
```

### let command

```bash
let "x = 5 + 3"
echo $x   # 8

let "x += 2"
echo $x   # 10

let "x++" "y = x * 2"
echo $x $y  # 11 22
```

### (( )) command

```bash
# Returns 0 (true) if expression is non-zero
if ((x > 5)); then
    echo "x is greater than 5"
fi

# Arithmetic for loop
for ((i=1; i<=5; i++)); do
    echo $i
done
```

## Pipelines and Redirections

```bash
# Pipeline
echo "hello world" | tr ' ' '\n' | sort

# Output redirection
echo "data" > /file.txt     # overwrite
echo "more" >> /file.txt    # append

# Input redirection
sort < /unsorted.txt

# Stderr redirection
command 2> /errors.log
command 2>&1                 # stderr to stdout

# Discard output
command > /dev/null 2>&1

# Here-documents
cat <<EOF
Hello, $USER!
Today is $(date).
EOF

# Here-strings
grep "pattern" <<< "search in this string"
```

## Subshells

```bash
# Parentheses create an isolated subshell
(cd /tmp && echo "In /tmp")
pwd  # still in original directory — subshell didn't affect parent

# Command substitution runs in a subshell too
result=$(echo hello | tr 'h' 'H')
echo $result  # Hello
```

## Brace Expansion

```bash
echo {a,b,c}           # a b c
echo file{1,2,3}.txt   # file1.txt file2.txt file3.txt
echo {1..5}            # 1 2 3 4 5
echo {01..03}          # 01 02 03 (zero-padded)
echo {a..f}            # a b c d e f
echo {1..10..2}        # 1 3 5 7 9
```

## Glob Patterns

```bash
echo *.txt              # all .txt files in cwd
echo /src/**/*.rs       # recursive glob
echo file?.log          # file1.log, fileA.log, etc.
echo [abc].txt          # a.txt, b.txt, c.txt
```

## Putting It All Together

A complete script using multiple features:

```bash
#!/bin/bash
# Process CSV data and generate a report

INPUT="/data/sales.csv"
OUTPUT="/data/report.txt"

# Validate input
if [ ! -f "$INPUT" ]; then
    echo "Error: $INPUT not found" >&2
    exit 1
fi

# Count records
total=$(tail -n +2 "$INPUT" | wc -l)

# Header
{
    echo "=== Sales Report ==="
    echo "Total records: $total"
    echo ""
    echo "Top 5 by amount:"
    tail -n +2 "$INPUT" | sort -t, -k3 -rn | head -5 | \
        awk -F, '{ printf "  %-20s $%s\n", $1, $3 }'
} > "$OUTPUT"

cat "$OUTPUT"
```

Use this in Rust:

```rust
use rust_bash::RustBashBuilder;
use std::collections::HashMap;

let script = r#"
INPUT="/data/sales.csv"
total=$(tail -n +2 "$INPUT" | wc -l)
echo "Total records: $total"
tail -n +2 "$INPUT" | sort -t, -k3 -rn | head -3 | \
    awk -F, '{ printf "%-15s $%s\n", $1, $3 }'
"#;

let mut shell = RustBashBuilder::new()
    .files(HashMap::from([
        ("/data/sales.csv".into(), b"name,region,amount\nAlice,East,50000\nBob,West,75000\nCarol,East,62000\n".to_vec()),
    ]))
    .build()
    .unwrap();

let result = shell.exec(script).unwrap();
assert!(result.stdout.contains("Total records: 3"));
```
