/// Smoke tests validating the brush-parser API surface.
///
/// These verify that `tokenize_str`, `parse_tokens`, and `word::parse` work
/// as expected and that the `WordPiece` variants match our guidebook assumptions.
///
/// **API differences from guidebook (Chapter 3)**:
/// - `word::parse()` takes `(&str, &ParserOptions)`, not a `WordParseOptions`.
/// - `word::parse()` returns `Vec<WordPieceWithSource>`, not `Vec<WordPiece>`.
///   Each element has a `.piece` field containing the `WordPiece`.
/// - The arithmetic variant is `ArithmeticExpression`, not `ArithmeticExpansion`.
/// - `parse_tokens()` takes 3 args: `(&Vec<Token>, &ParserOptions, &SourceInfo)`.

fn default_parser_options() -> brush_parser::ParserOptions {
    brush_parser::ParserOptions {
        sh_mode: false,
        ..Default::default()
    }
}

fn source_info() -> brush_parser::SourceInfo {
    brush_parser::SourceInfo {
        source: String::new(),
    }
}

#[test]
fn tokenize_simple_command() {
    let tokens = brush_parser::tokenize_str("echo hello world").unwrap();
    assert!(!tokens.is_empty(), "tokenize_str returned no tokens");
}

#[test]
fn parse_simple_command() {
    let tokens = brush_parser::tokenize_str("echo hello").unwrap();
    let program =
        brush_parser::parse_tokens(&tokens, &default_parser_options(), &source_info()).unwrap();
    assert!(
        !program.complete_commands.is_empty(),
        "parsed program has no commands"
    );
}

#[test]
fn parse_pipeline() {
    let tokens = brush_parser::tokenize_str("cat file.txt | grep pattern | wc -l").unwrap();
    let program =
        brush_parser::parse_tokens(&tokens, &default_parser_options(), &source_info()).unwrap();
    assert!(!program.complete_commands.is_empty());
}

#[test]
fn parse_compound_commands() {
    let inputs = [
        "if true; then echo yes; fi",
        "for x in a b c; do echo $x; done",
        "while true; do break; done",
        "{ echo a; echo b; }",
        "(echo subshell)",
    ];
    let opts = default_parser_options();
    let si = source_info();
    for input in &inputs {
        let tokens = brush_parser::tokenize_str(input).unwrap();
        let program = brush_parser::parse_tokens(&tokens, &opts, &si).unwrap();
        assert!(
            !program.complete_commands.is_empty(),
            "failed to parse: {input}"
        );
    }
}

#[test]
fn word_parse_literal() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("hello", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::Text(s) => assert_eq!(s, "hello"),
        other => panic!("expected Text, got {other:?}"),
    }
}

#[test]
fn word_parse_single_quoted() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("'no expansion'", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::SingleQuotedText(s) => {
            assert_eq!(s, "no expansion");
        }
        other => panic!("expected SingleQuotedText, got {other:?}"),
    }
}

#[test]
fn word_parse_double_quoted_with_expansion() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("\"hello $USER\"", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::DoubleQuotedSequence(inner) => {
            assert!(
                inner.len() >= 2,
                "expected at least 2 pieces inside double quote, got {inner:?}"
            );
        }
        other => panic!("expected DoubleQuotedSequence, got {other:?}"),
    }
}

#[test]
fn word_parse_command_substitution() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("$(echo hi)", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::CommandSubstitution(_) => {}
        other => panic!("expected CommandSubstitution, got {other:?}"),
    }
}

#[test]
fn word_parse_backtick_substitution() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("`echo hi`", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::BackquotedCommandSubstitution(_) => {}
        other => panic!("expected BackquotedCommandSubstitution, got {other:?}"),
    }
}

#[test]
fn word_parse_arithmetic_expression() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("$((1+2))", &opts).unwrap();
    assert!(!pieces.is_empty());
    // NOTE: brush-parser uses `ArithmeticExpression`, not `ArithmeticExpansion`
    match &pieces[0].piece {
        brush_parser::word::WordPiece::ArithmeticExpression(_) => {}
        other => panic!("expected ArithmeticExpression, got {other:?}"),
    }
}

#[test]
fn word_parse_tilde() {
    let mut opts = default_parser_options();
    opts.tilde_expansion = true;
    let pieces = brush_parser::word::parse("~/bin", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::TildePrefix(s) => {
            assert_eq!(s, "");
        }
        other => panic!("expected TildePrefix, got {other:?}"),
    }
}

#[test]
fn word_parse_parameter_expansion_braced() {
    let opts = default_parser_options();
    let pieces = brush_parser::word::parse("${VAR:-default}", &opts).unwrap();
    assert!(!pieces.is_empty());
    match &pieces[0].piece {
        brush_parser::word::WordPiece::ParameterExpansion(_) => {}
        other => panic!("expected ParameterExpansion, got {other:?}"),
    }
}

/// Compile-time verification that expected WordPiece variant names exist.
#[test]
fn wordpiece_variant_names_exist() {
    let opts = default_parser_options();

    let check = |input: &str, variant_name: &str| {
        let pieces = brush_parser::word::parse(input, &opts).unwrap();
        assert!(
            !pieces.is_empty(),
            "no pieces for input: {input} (expected {variant_name})"
        );
    };

    check("hello", "Text");
    check("'quoted'", "SingleQuotedText");
    check("$(cmd)", "CommandSubstitution");
    check("$((1+2))", "ArithmeticExpression");
    check("${X}", "ParameterExpansion");
}
