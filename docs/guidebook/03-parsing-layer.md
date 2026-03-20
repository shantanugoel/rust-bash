# Chapter 3: Parsing Layer

## Overview

rust-bash does not implement its own bash parser. Instead, it uses [brush-parser](https://github.com/reubeno/brush), a standalone Rust crate that provides a complete bash grammar parser. This gives us a battle-tested parser for free and lets us focus entirely on execution semantics.

## Parsing Pipeline

```
Raw command string
       │
       ▼
tokenize_str(input)
       │ Vec<Token>
       ▼
parse_tokens(&tokens, &options)
       │ Program (AST)
       ▼
Interpreter walks AST
```

### Step 1: Tokenization

`brush_parser::tokenize_str()` splits the raw input into tokens according to bash lexical rules. This handles:
- Word boundaries (whitespace, operators)
- Quoting (single quotes, double quotes, backslash escaping)
- Operator recognition (`|`, `&&`, `||`, `;`, `>`, `>>`, `<`, `<<`, etc.)
- Comment stripping (`#` to end of line)
- Here-document body capture

### Step 2: Parsing

`brush_parser::parse_tokens()` builds an AST from the token stream. The parser handles the full bash grammar including:
- Simple commands, pipelines, and lists
- Compound commands (`if`, `for`, `while`, `until`, `case`, `{ }`, `( )`)
- Function definitions
- Redirections and here-documents
- Arithmetic expressions `$(( ))`
- Extended test expressions `[[ ]]`

### Step 3: Word Expansion (Deferred)

Word expansion is *not* done during parsing. The parser produces `Word` nodes containing raw text. At execution time, the interpreter calls `brush_parser::word::parse()` to decompose each word into expansion pieces:

```
"hello $USER"
       │
       ▼
word::parse("\"hello $USER\"", &options)
       │
       ▼
[DoubleQuotedSequence([
    Text("hello "),
    ParameterExpansion(Named("USER"))
])]
```

This decomposition is the single biggest reuse win from brush-parser. Parsing word syntax (nested quoting, parameter expansion syntax, command substitution delimiters, arithmetic expressions inside words) is extremely complex. brush-parser handles all of it.

## AST Types

The key AST types we depend on, **simplified for readability**. Actual types use wrapper structs, tuple variants, and additional fields — see `brush-parser/src/ast.rs` for the full definitions.

```
Program
  └── complete_commands: Vec<CompleteCommand>

CompleteCommand
  └── list: CompoundList, separator: Option<SeparatorOperator>

CompoundList
  └── Vec<CompoundListItem>

CompoundListItem(AndOrList, SeparatorOperator)   // tuple struct

AndOrList
  ├── first: Pipeline
  └── additional: Vec<(AndOr, Pipeline)>  // && or ||

Pipeline
  ├── bang: bool          // ! prefix (negate exit code)
  └── seq: Vec<Command>   // piped together

Command
  ├── Simple(SimpleCommand)
  ├── Compound(CompoundCommand, Option<RedirectList>)
  ├── Function(FunctionDefinition)
  └── ExtendedTest(ExtendedTestExprCommand, Option<RedirectList>)

SimpleCommand
  ├── prefix: Option<CommandPrefix>      // assignments and redirections
  ├── word_or_name: Option<Word>         // command name
  └── suffix: Option<CommandSuffix>      // arguments and redirections

CompoundCommand (each variant wraps a dedicated struct)
  ├── BraceGroup(BraceGroupCommand)
  ├── Subshell(SubshellCommand)
  ├── ForClause(ForClauseCommand)
  ├── ArithmeticForClause(ArithmeticForClauseCommand)
  ├── WhileClause(WhileClauseCommand)
  ├── UntilClause(UntilClauseCommand)
  ├── IfClause(IfClauseCommand)
  ├── CaseClause(CaseClauseCommand)
  └── Arithmetic(ArithmeticCommand)
```

## WordPiece Types

`brush_parser::word::parse()` decomposes a word into these piece types. Less common variants (e.g., `AnsiCQuotedText`, `EscapeSequence`, `GettextDoubleQuotedSequence`) are omitted.

> **Note (brush-parser 0.3.0 API):** `word::parse()` takes `(&str, &ParserOptions)` and returns
> `Vec<WordPieceWithSource>`. Each element has a `.piece: WordPiece` field. The `DoubleQuotedSequence`
> variant wraps `Vec<WordPieceWithSource>`, not `Vec<WordPiece>`. The arithmetic variant is
> `ArithmeticExpression(ast::UnexpandedArithmeticExpr)`, not `ArithmeticExpansion(String)`.

| Piece | Example | Description |
|-------|---------|-------------|
| `Text(String)` | `hello` | Literal text |
| `SingleQuotedText(String)` | `'no expansion'` | Literal, no expansion |
| `DoubleQuotedSequence(Vec<WordPieceWithSource>)` | `"hello $x"` | Sequence of pieces, expanded but not word-split |
| `ParameterExpansion(ParameterExpr)` | `$VAR`, `${VAR:-default}` | Variable reference with optional operators (complex enum) |
| `CommandSubstitution(String)` | `$(cmd)` | Execute command, capture stdout |
| `BackquotedCommandSubstitution(String)` | `` `cmd` `` | Legacy syntax for command substitution |
| `ArithmeticExpression(UnexpandedArithmeticExpr)` | `$((1+2))` | Evaluate arithmetic expression |
| `TildePrefix(String)` | `~`, `~user` | Expand to home directory |

## Redirection Types

The parser produces `IoRedirect` nodes for redirections. The actual enum has four variants: `File`, `HereDocument`, `HereString`, and `OutputAndError`. File redirections use `IoFileRedirectKind` to distinguish the operation type.

| Syntax | Semantic Description | Behavior |
|--------|---------------------|----------|
| `> file` | File redirect, write (fd 1) | Write stdout to file |
| `>> file` | File redirect, append (fd 1) | Append stdout to file |
| `< file` | File redirect, read (fd 0) | Read stdin from file |
| `2> file` | File redirect, write (fd 2) | Write stderr to file |
| `2>&1` | File redirect, duplicate output (fd 2 → 1) | Redirect stderr to stdout |
| `<<EOF` | HereDocument | Multi-line stdin from literal text |
| `<<<word` | HereString | Single-line stdin from word expansion |
| `&> file` | OutputAndError | Redirect both stdout and stderr to file |

## Parser Configuration

> **Note (brush-parser 0.3.0 API):** `parse_tokens()` takes three arguments:
> `(&Vec<Token>, &ParserOptions, &SourceInfo)`. The `SourceInfo` struct has a single `source: String`
> field. The field `tilde_expansion_at_word_start` was renamed to `tilde_expansion`, and
> `tilde_expansion_after_colon` was removed.

```rust
let parse_options = brush_parser::ParserOptions {
    sh_mode: false,                       // bash mode, not POSIX sh
    posix_mode: false,                    // allow bash extensions
    enable_extended_globbing: true,       // @(...), +(...), etc.
    tilde_expansion: true,                // ~ → $HOME
    ..Default::default()
};
```

We parse in bash mode with extended globbing enabled. POSIX sh mode disables bash-specific features like `[[ ]]`, `(( ))`, and brace expansion.

## Handling Parser Errors

brush-parser returns `Result` from both tokenization and parsing. Parse errors are wrapped into `RustBashError::Parse` with the original error message. The interpreter does not attempt error recovery — a parse failure stops execution immediately (matching bash behavior with `set -e` or a syntax error in a non-interactive script).

## Dependency Pinning

brush-parser is available on crates.io:

```toml
[dependencies]
brush-parser = "0.3.0"
```

When upgrading, run the full test suite and check for AST type changes. Consider wrapping brush-parser types in adapter types if upstream churn becomes a problem.
