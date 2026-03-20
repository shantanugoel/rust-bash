use super::parser::{
    AssignOp, AwkPattern, AwkProgram, AwkRule, AwkStatement, BinOp, Expr, UnaryOp,
};
use regex::Regex;
use std::collections::HashMap;

// ── Control flow signals ────────────────────────────────────────────────

enum Signal {
    None,
    Break,
    Continue,
    Next,
    Exit(i32),
}

// ── Awk value type ──────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub enum AwkValue {
    Str(String),
    Num(f64),
    Uninitialized,
}

impl AwkValue {
    pub fn to_num(&self) -> f64 {
        match self {
            AwkValue::Num(n) => *n,
            AwkValue::Str(s) => parse_awk_number(s),
            AwkValue::Uninitialized => 0.0,
        }
    }

    pub fn to_string_val(&self) -> String {
        match self {
            AwkValue::Str(s) => s.clone(),
            AwkValue::Num(n) => format_number(*n),
            AwkValue::Uninitialized => String::new(),
        }
    }

    pub fn is_true(&self) -> bool {
        match self {
            AwkValue::Num(n) => *n != 0.0,
            AwkValue::Str(s) => !s.is_empty(),
            AwkValue::Uninitialized => false,
        }
    }
}

fn format_number(n: f64) -> String {
    if n == n.trunc() && n.abs() < 1e16 && !n.is_infinite() {
        // Integer-like: print without decimal
        format!("{}", n as i64)
    } else if n.is_infinite() {
        if n > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        }
    } else if n.is_nan() {
        "nan".to_string()
    } else {
        // Use %g-like formatting (6 significant digits)
        format_g(n, 6)
    }
}

fn parse_awk_number(s: &str) -> f64 {
    let s = s.trim();
    if s.is_empty() {
        return 0.0;
    }
    // Parse leading numeric portion
    let mut end = 0;
    let bytes = s.as_bytes();
    if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
        end += 1;
    }
    let mut has_digit = false;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
        has_digit = true;
    }
    if end < bytes.len() && bytes[end] == b'.' {
        end += 1;
        while end < bytes.len() && bytes[end].is_ascii_digit() {
            end += 1;
            has_digit = true;
        }
    }
    if has_digit && end < bytes.len() && (bytes[end] == b'e' || bytes[end] == b'E') {
        let saved = end;
        end += 1;
        if end < bytes.len() && (bytes[end] == b'+' || bytes[end] == b'-') {
            end += 1;
        }
        if end < bytes.len() && bytes[end].is_ascii_digit() {
            while end < bytes.len() && bytes[end].is_ascii_digit() {
                end += 1;
            }
        } else {
            end = saved;
        }
    }
    if !has_digit {
        return 0.0;
    }
    s[..end].parse().unwrap_or(0.0)
}

// ── Runtime ─────────────────────────────────────────────────────────────

pub struct AwkRuntime {
    // Built-in variables
    pub variables: HashMap<String, AwkValue>,
    // Associative arrays
    pub arrays: HashMap<String, HashMap<String, AwkValue>>,
    // Fields: $0, $1, $2, etc.
    fields: Vec<String>,
    // Output
    pub stdout: String,
    pub stderr: String,
    // State
    nr: i64,
    fnr: i64,
    filename: String,
    // Regex cache
    regex_cache: HashMap<String, Regex>,
    // Range pattern state: track whether each range is active
    range_active: Vec<bool>,
    // RNG state
    rng_state: u64,
    // Exit code
    exit_code: i32,
    // Execution limits
    max_loop_iterations: usize,
    max_output_size: usize,
    max_field_index: usize,
}

impl AwkRuntime {
    pub fn new() -> Self {
        let mut variables = HashMap::new();
        variables.insert("FS".to_string(), AwkValue::Str(" ".to_string()));
        variables.insert("OFS".to_string(), AwkValue::Str(" ".to_string()));
        variables.insert("RS".to_string(), AwkValue::Str("\n".to_string()));
        variables.insert("ORS".to_string(), AwkValue::Str("\n".to_string()));
        variables.insert("NR".to_string(), AwkValue::Num(0.0));
        variables.insert("NF".to_string(), AwkValue::Num(0.0));
        variables.insert("FNR".to_string(), AwkValue::Num(0.0));
        variables.insert("RSTART".to_string(), AwkValue::Num(0.0));
        variables.insert("RLENGTH".to_string(), AwkValue::Num(-1.0));
        variables.insert("SUBSEP".to_string(), AwkValue::Str("\x1c".to_string()));
        variables.insert("FILENAME".to_string(), AwkValue::Str(String::new()));

        Self {
            variables,
            arrays: HashMap::new(),
            fields: vec![String::new()],
            stdout: String::new(),
            stderr: String::new(),
            nr: 0,
            fnr: 0,
            filename: String::new(),
            regex_cache: HashMap::new(),
            range_active: Vec::new(),
            rng_state: 0,
            exit_code: 0,
            max_loop_iterations: 10_000_000,
            max_output_size: 10 * 1024 * 1024,
            max_field_index: 10_000,
        }
    }

    pub fn apply_limits(&mut self, limits: &crate::interpreter::ExecutionLimits) {
        self.max_loop_iterations = limits.max_loop_iterations;
        self.max_output_size = limits.max_output_size;
    }

    pub fn set_var(&mut self, name: &str, value: &str) {
        let val = if let Ok(n) = value.parse::<f64>() {
            if n.is_finite() {
                AwkValue::Num(n)
            } else {
                AwkValue::Str(value.to_string())
            }
        } else {
            AwkValue::Str(value.to_string())
        };
        self.variables.insert(name.to_string(), val);
    }

    pub fn set_argc_argv(&mut self, args: &[String]) {
        self.variables
            .insert("ARGC".to_string(), AwkValue::Num(args.len() as f64));
        let argv = self.arrays.entry("ARGV".to_string()).or_default();
        for (i, arg) in args.iter().enumerate() {
            argv.insert(i.to_string(), AwkValue::Str(arg.clone()));
        }
    }

    pub fn execute(
        &mut self,
        program: &AwkProgram,
        inputs: &[(String, String)],
    ) -> (i32, String, String) {
        // Initialize range_active for all range patterns
        self.range_active = vec![false; program.rules.len()];

        // Execute BEGIN rules
        for rule in &program.rules {
            if matches!(rule.pattern, Some(AwkPattern::Begin))
                && let Some(action) = &rule.action
                && let Signal::Exit(code) = self.execute_block(action)
            {
                return (code, self.stdout.clone(), self.stderr.clone());
            }
        }

        // Process each input
        if inputs.is_empty() {
            // No input files and no stdin content: skip to END
        } else {
            'outer: for (filename, content) in inputs {
                self.fnr = 0;
                self.filename = filename.clone();
                self.variables
                    .insert("FILENAME".to_string(), AwkValue::Str(filename.clone()));

                let rs = self.get_var("RS").to_string_val();
                let records = split_records(content, &rs);

                'record: for record in &records {
                    self.nr += 1;
                    self.fnr += 1;
                    self.set_record(record);
                    self.sync_builtin_vars();

                    for (rule_idx, rule) in program.rules.iter().enumerate() {
                        if matches!(rule.pattern, Some(AwkPattern::Begin | AwkPattern::End)) {
                            continue;
                        }

                        let matched = match self.pattern_matches(rule, rule_idx) {
                            Ok(m) => m,
                            Err(()) => continue,
                        };
                        if !matched {
                            continue;
                        }

                        let default_action = [AwkStatement::Print { exprs: vec![] }];
                        let action = rule.action.as_deref().unwrap_or(&default_action);

                        match self.execute_block(action) {
                            Signal::Next => continue 'record,
                            Signal::Exit(code) => {
                                self.exit_code = code;
                                break 'outer;
                            }
                            Signal::Break | Signal::Continue => {}
                            Signal::None => {}
                        }
                    }
                }
            }
        }

        // Execute END rules
        for rule in &program.rules {
            if matches!(rule.pattern, Some(AwkPattern::End))
                && let Some(action) = &rule.action
            {
                self.sync_builtin_vars();
                if let Signal::Exit(code) = self.execute_block(action) {
                    self.exit_code = code;
                    break;
                }
            }
        }

        (self.exit_code, self.stdout.clone(), self.stderr.clone())
    }

    fn pattern_matches(&mut self, rule: &AwkRule, rule_idx: usize) -> Result<bool, ()> {
        match &rule.pattern {
            None => Ok(true),
            Some(AwkPattern::Begin | AwkPattern::End) => Ok(false),
            Some(AwkPattern::Regex(r)) => {
                let field0 = self.fields[0].clone();
                Ok(self.regex_match(r, &field0))
            }
            Some(AwkPattern::Expression(expr)) => {
                let val = self.eval_expr(expr);
                Ok(val.is_true())
            }
            Some(AwkPattern::Range(start, end)) => {
                let active = self.range_active.get(rule_idx).copied().unwrap_or(false);
                if active {
                    let end_val = self.eval_expr(end);
                    if end_val.is_true()
                        && let Some(a) = self.range_active.get_mut(rule_idx)
                    {
                        *a = false;
                    }
                    Ok(true)
                } else {
                    let start_val = self.eval_expr(start);
                    if start_val.is_true() {
                        if let Some(a) = self.range_active.get_mut(rule_idx) {
                            *a = true;
                        }
                        let end_val = self.eval_expr(end);
                        if end_val.is_true()
                            && let Some(a) = self.range_active.get_mut(rule_idx)
                        {
                            *a = false;
                        }
                        Ok(true)
                    } else {
                        Ok(false)
                    }
                }
            }
        }
    }

    // ── Record & field management ────────────────────────────────────

    fn set_record(&mut self, record: &str) {
        self.fields = vec![record.to_string()];
        let fs = self.get_var("FS").to_string_val();
        let split_fields = split_fields(record, &fs);
        self.fields.extend(split_fields);
        self.variables.insert(
            "NF".to_string(),
            AwkValue::Num((self.fields.len() - 1) as f64),
        );
    }

    fn rebuild_record(&mut self) {
        let ofs = self.get_var("OFS").to_string_val();
        let nf = self.fields.len() - 1;
        if nf == 0 {
            self.fields[0] = String::new();
        } else {
            self.fields[0] = self.fields[1..].join(&ofs);
        }
    }

    fn sync_builtin_vars(&mut self) {
        self.variables
            .insert("NR".to_string(), AwkValue::Num(self.nr as f64));
        self.variables
            .insert("FNR".to_string(), AwkValue::Num(self.fnr as f64));
    }

    fn get_field(&self, idx: usize) -> String {
        if idx < self.fields.len() {
            self.fields[idx].clone()
        } else {
            String::new()
        }
    }

    fn set_field(&mut self, idx: usize, value: &str) {
        if idx > self.max_field_index {
            self.stderr.push_str(&format!(
                "awk: field index {idx} exceeds limit {}\n",
                self.max_field_index
            ));
            return;
        }
        while self.fields.len() <= idx {
            self.fields.push(String::new());
        }
        self.fields[idx] = value.to_string();
        if idx == 0 {
            // Re-split fields from $0
            let fs = self.get_var("FS").to_string_val();
            let split_fields = split_fields(value, &fs);
            self.fields.truncate(1);
            self.fields.extend(split_fields);
        } else {
            // Rebuild $0
            self.rebuild_record();
        }
        self.variables.insert(
            "NF".to_string(),
            AwkValue::Num((self.fields.len() - 1) as f64),
        );
    }

    // ── Variable access ──────────────────────────────────────────────

    fn get_var(&self, name: &str) -> AwkValue {
        self.variables
            .get(name)
            .cloned()
            .unwrap_or(AwkValue::Uninitialized)
    }

    fn set_variable(&mut self, name: &str, value: AwkValue) {
        self.variables.insert(name.to_string(), value);
        // If NF is set, adjust fields count
        if name == "NF" {
            let nf = self
                .variables
                .get("NF")
                .map(|v| v.to_num() as usize)
                .unwrap_or(0);
            let current_nf = self.fields.len() - 1;
            if nf < current_nf {
                self.fields.truncate(nf + 1);
            } else {
                while self.fields.len() <= nf {
                    self.fields.push(String::new());
                }
            }
            self.rebuild_record();
        }
        // If FS changes, we don't re-split current record (matches awk behavior)
    }

    fn get_array_val(&mut self, name: &str, key: &str) -> AwkValue {
        self.arrays
            .get(name)
            .and_then(|a| a.get(key))
            .cloned()
            .unwrap_or(AwkValue::Uninitialized)
    }

    fn set_array_val(&mut self, name: &str, key: &str, value: AwkValue) {
        self.arrays
            .entry(name.to_string())
            .or_default()
            .insert(key.to_string(), value);
    }

    // ── Block / statement execution ──────────────────────────────────

    fn execute_block(&mut self, stmts: &[AwkStatement]) -> Signal {
        for stmt in stmts {
            let sig = self.execute_statement(stmt);
            match sig {
                Signal::None => {}
                other => return other,
            }
        }
        Signal::None
    }

    fn execute_statement(&mut self, stmt: &AwkStatement) -> Signal {
        match stmt {
            AwkStatement::Print { exprs } => {
                self.exec_print(exprs);
                Signal::None
            }
            AwkStatement::Printf { format, exprs } => {
                self.exec_printf(format, exprs);
                Signal::None
            }
            AwkStatement::If { cond, then, else_ } => {
                let val = self.eval_expr(cond);
                if val.is_true() {
                    self.execute_statement(then)
                } else if let Some(e) = else_ {
                    self.execute_statement(e)
                } else {
                    Signal::None
                }
            }
            AwkStatement::While { cond, body } => {
                let mut iterations = 0usize;
                loop {
                    iterations += 1;
                    if iterations > self.max_loop_iterations {
                        self.stderr.push_str("awk: loop iteration limit exceeded\n");
                        break;
                    }
                    let val = self.eval_expr(cond);
                    if !val.is_true() {
                        break;
                    }
                    match self.execute_statement(body) {
                        Signal::Break => break,
                        Signal::Continue => continue,
                        Signal::Next => return Signal::Next,
                        Signal::Exit(c) => return Signal::Exit(c),
                        Signal::None => {}
                    }
                }
                Signal::None
            }
            AwkStatement::DoWhile { body, cond } => {
                let mut iterations = 0usize;
                loop {
                    iterations += 1;
                    if iterations > self.max_loop_iterations {
                        self.stderr.push_str("awk: loop iteration limit exceeded\n");
                        break;
                    }
                    match self.execute_statement(body) {
                        Signal::Break => break,
                        Signal::Continue => {}
                        Signal::Next => return Signal::Next,
                        Signal::Exit(c) => return Signal::Exit(c),
                        Signal::None => {}
                    }
                    let val = self.eval_expr(cond);
                    if !val.is_true() {
                        break;
                    }
                }
                Signal::None
            }
            AwkStatement::For {
                init,
                cond,
                step,
                body,
            } => {
                if let Some(init) = init {
                    let sig = self.execute_statement(init);
                    if !matches!(sig, Signal::None) {
                        return sig;
                    }
                }
                let mut iterations = 0usize;
                loop {
                    iterations += 1;
                    if iterations > self.max_loop_iterations {
                        self.stderr.push_str("awk: loop iteration limit exceeded\n");
                        break;
                    }
                    if let Some(cond) = cond {
                        let val = self.eval_expr(cond);
                        if !val.is_true() {
                            break;
                        }
                    }
                    match self.execute_statement(body) {
                        Signal::Break => break,
                        Signal::Continue => {}
                        Signal::Next => return Signal::Next,
                        Signal::Exit(c) => return Signal::Exit(c),
                        Signal::None => {}
                    }
                    if let Some(step) = step {
                        self.execute_statement(step);
                    }
                }
                Signal::None
            }
            AwkStatement::ForIn { var, array, body } => {
                let keys: Vec<String> = self
                    .arrays
                    .get(array.as_str())
                    .map(|a| a.keys().cloned().collect())
                    .unwrap_or_default();
                let mut iterations = 0usize;
                for key in keys {
                    iterations += 1;
                    if iterations > self.max_loop_iterations {
                        self.stderr.push_str("awk: loop iteration limit exceeded\n");
                        break;
                    }
                    self.set_variable(var, AwkValue::Str(key));
                    match self.execute_statement(body) {
                        Signal::Break => break,
                        Signal::Continue => continue,
                        Signal::Next => return Signal::Next,
                        Signal::Exit(c) => return Signal::Exit(c),
                        Signal::None => {}
                    }
                }
                Signal::None
            }
            AwkStatement::Block(stmts) => self.execute_block(stmts),
            AwkStatement::Expression(expr) => {
                self.eval_expr(expr);
                Signal::None
            }
            AwkStatement::Break => Signal::Break,
            AwkStatement::Continue => Signal::Continue,
            AwkStatement::Next => Signal::Next,
            AwkStatement::Exit(code) => {
                let c = code
                    .as_ref()
                    .map(|e| self.eval_expr(e).to_num() as i32)
                    .unwrap_or(0);
                Signal::Exit(c)
            }
            AwkStatement::Delete { array, indices } => {
                if let Some(indices) = indices {
                    let key = self.eval_array_key(indices);
                    if let Some(arr) = self.arrays.get_mut(array.as_str()) {
                        arr.remove(&key);
                    }
                } else {
                    self.arrays.remove(array.as_str());
                }
                Signal::None
            }
        }
    }

    // ── Print / printf ───────────────────────────────────────────────

    fn output_limit_reached(&self) -> bool {
        self.stdout.len() > self.max_output_size
    }

    fn exec_print(&mut self, exprs: &[Expr]) {
        if self.output_limit_reached() {
            return;
        }
        let ors = self.get_var("ORS").to_string_val();
        if exprs.is_empty() {
            let field0 = self.get_field(0);
            self.stdout.push_str(&field0);
        } else {
            let ofs = self.get_var("OFS").to_string_val();
            let mut parts = Vec::new();
            for expr in exprs {
                parts.push(self.eval_expr(expr).to_string_val());
            }
            self.stdout.push_str(&parts.join(&ofs));
        }
        self.stdout.push_str(&ors);
    }

    fn exec_printf(&mut self, format_expr: &Expr, arg_exprs: &[Expr]) {
        if self.output_limit_reached() {
            return;
        }
        let fmt = self.eval_expr(format_expr).to_string_val();
        let args: Vec<AwkValue> = arg_exprs.iter().map(|e| self.eval_expr(e)).collect();
        let result = awk_sprintf(&fmt, &args);
        self.stdout.push_str(&result);
    }

    // ── Expression evaluation ────────────────────────────────────────

    fn eval_expr(&mut self, expr: &Expr) -> AwkValue {
        match expr {
            Expr::Number(n) => AwkValue::Num(*n),
            Expr::String(s) => AwkValue::Str(s.clone()),
            Expr::Regex(r) => {
                // Regex in expression context: match against $0
                let field0 = self.get_field(0);
                let matched = self.regex_match(r, &field0);
                AwkValue::Num(if matched { 1.0 } else { 0.0 })
            }
            Expr::Var(name) => self.get_var(name),
            Expr::FieldRef(idx_expr) => {
                let idx = self.eval_expr(idx_expr).to_num() as usize;
                let val = self.get_field(idx);
                AwkValue::Str(val)
            }
            Expr::ArrayRef { name, indices } => {
                let key = self.eval_array_key(indices);
                self.get_array_val(name, &key)
            }
            Expr::BinaryOp { op, left, right } => self.eval_binary_op(*op, left, right),
            Expr::UnaryOp { op, expr } => {
                let val = self.eval_expr(expr);
                match op {
                    UnaryOp::Neg => AwkValue::Num(-val.to_num()),
                    UnaryOp::Pos => AwkValue::Num(val.to_num()),
                    UnaryOp::Not => AwkValue::Num(if val.is_true() { 0.0 } else { 1.0 }),
                }
            }
            Expr::Assign { target, op, value } => self.eval_assign(target, *op, value),
            Expr::Ternary { cond, then, else_ } => {
                if self.eval_expr(cond).is_true() {
                    self.eval_expr(then)
                } else {
                    self.eval_expr(else_)
                }
            }
            Expr::FuncCall { name, args } => self.eval_func_call(name, args),
            Expr::Concat { left, right } => {
                let l = self.eval_expr(left).to_string_val();
                let r = self.eval_expr(right).to_string_val();
                AwkValue::Str(format!("{l}{r}"))
            }
            Expr::InArray { index, array } => {
                let key = self.eval_expr(index).to_string_val();
                let exists = self
                    .arrays
                    .get(array.as_str())
                    .is_some_and(|a| a.contains_key(&key));
                AwkValue::Num(if exists { 1.0 } else { 0.0 })
            }
            Expr::Match {
                expr,
                regex,
                negated,
            } => {
                let s = self.eval_expr(expr).to_string_val();
                let pattern = match regex.as_ref() {
                    Expr::Regex(r) => r.clone(),
                    other => self.eval_expr(other).to_string_val(),
                };
                let matched = self.regex_match(&pattern, &s);
                let result = if *negated { !matched } else { matched };
                AwkValue::Num(if result { 1.0 } else { 0.0 })
            }
            Expr::PreIncrement(e) => {
                let val = self.eval_expr(e).to_num() + 1.0;
                self.assign_to(e, AwkValue::Num(val));
                AwkValue::Num(val)
            }
            Expr::PreDecrement(e) => {
                let val = self.eval_expr(e).to_num() - 1.0;
                self.assign_to(e, AwkValue::Num(val));
                AwkValue::Num(val)
            }
            Expr::PostIncrement(e) => {
                let val = self.eval_expr(e).to_num();
                self.assign_to(e, AwkValue::Num(val + 1.0));
                AwkValue::Num(val)
            }
            Expr::PostDecrement(e) => {
                let val = self.eval_expr(e).to_num();
                self.assign_to(e, AwkValue::Num(val - 1.0));
                AwkValue::Num(val)
            }
            Expr::Getline => {
                // Basic getline: not fully supported, return 0
                AwkValue::Num(0.0)
            }
        }
    }

    fn eval_binary_op(&mut self, op: BinOp, left: &Expr, right: &Expr) -> AwkValue {
        match op {
            BinOp::And => {
                let l = self.eval_expr(left);
                if !l.is_true() {
                    return AwkValue::Num(0.0);
                }
                let r = self.eval_expr(right);
                AwkValue::Num(if r.is_true() { 1.0 } else { 0.0 })
            }
            BinOp::Or => {
                let l = self.eval_expr(left);
                if l.is_true() {
                    return AwkValue::Num(1.0);
                }
                let r = self.eval_expr(right);
                AwkValue::Num(if r.is_true() { 1.0 } else { 0.0 })
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Div | BinOp::Mod | BinOp::Pow => {
                let l = self.eval_expr(left).to_num();
                let r = self.eval_expr(right).to_num();
                let result = match op {
                    BinOp::Add => l + r,
                    BinOp::Sub => l - r,
                    BinOp::Mul => l * r,
                    BinOp::Div => {
                        if r == 0.0 {
                            self.stderr.push_str("awk: division by zero\n");
                            0.0
                        } else {
                            l / r
                        }
                    }
                    BinOp::Mod => {
                        if r == 0.0 {
                            self.stderr.push_str("awk: division by zero\n");
                            0.0
                        } else {
                            l % r
                        }
                    }
                    BinOp::Pow => l.powf(r),
                    _ => unreachable!(),
                };
                AwkValue::Num(result)
            }
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge | BinOp::Eq | BinOp::Ne => {
                let l = self.eval_expr(left);
                let r = self.eval_expr(right);
                let result = compare_values(&l, &r, op);
                AwkValue::Num(if result { 1.0 } else { 0.0 })
            }
        }
    }

    fn eval_assign(&mut self, target: &Expr, op: AssignOp, value: &Expr) -> AwkValue {
        let new_val = if op == AssignOp::Assign {
            self.eval_expr(value)
        } else {
            let old = self.eval_expr(target).to_num();
            let rhs = self.eval_expr(value).to_num();
            let result = match op {
                AssignOp::AddAssign => old + rhs,
                AssignOp::SubAssign => old - rhs,
                AssignOp::MulAssign => old * rhs,
                AssignOp::DivAssign => {
                    if rhs == 0.0 {
                        self.stderr.push_str("awk: division by zero\n");
                        0.0
                    } else {
                        old / rhs
                    }
                }
                AssignOp::ModAssign => {
                    if rhs == 0.0 {
                        self.stderr.push_str("awk: division by zero\n");
                        0.0
                    } else {
                        old % rhs
                    }
                }
                AssignOp::PowAssign => old.powf(rhs),
                AssignOp::Assign => unreachable!(),
            };
            AwkValue::Num(result)
        };
        self.assign_to(target, new_val.clone());
        new_val
    }

    fn assign_to(&mut self, target: &Expr, value: AwkValue) {
        match target {
            Expr::Var(name) => {
                self.set_variable(name, value);
            }
            Expr::FieldRef(idx_expr) => {
                let idx = self.eval_expr(idx_expr).to_num() as usize;
                self.set_field(idx, &value.to_string_val());
            }
            Expr::ArrayRef { name, indices } => {
                let key = self.eval_array_key(indices);
                self.set_array_val(name, &key, value);
            }
            _ => {
                // Invalid assignment target — silently ignore
            }
        }
    }

    fn eval_array_key(&mut self, indices: &[Expr]) -> String {
        if indices.len() == 1 {
            self.eval_expr(&indices[0]).to_string_val()
        } else {
            let subsep = self.get_var("SUBSEP").to_string_val();
            indices
                .iter()
                .map(|e| self.eval_expr(e).to_string_val())
                .collect::<Vec<_>>()
                .join(&subsep)
        }
    }

    // ── Built-in functions ───────────────────────────────────────────

    fn eval_func_call(&mut self, name: &str, args: &[Expr]) -> AwkValue {
        match name {
            "length" => {
                if args.is_empty() {
                    let s = self.get_field(0);
                    AwkValue::Num(s.chars().count() as f64)
                } else {
                    let val = self.eval_expr(&args[0]);
                    if let Expr::Var(vname) = &args[0]
                        && let Some(arr) = self.arrays.get(vname.as_str())
                    {
                        return AwkValue::Num(arr.len() as f64);
                    }
                    AwkValue::Num(val.to_string_val().chars().count() as f64)
                }
            }
            "substr" => {
                if args.len() < 2 {
                    return AwkValue::Str(String::new());
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                let chars: Vec<char> = s.chars().collect();
                let start = self.eval_expr(&args[1]).to_num() as i64;
                // awk substr is 1-based
                let start_idx = (start - 1).max(0) as usize;
                if start_idx >= chars.len() {
                    return AwkValue::Str(String::new());
                }
                if args.len() >= 3 {
                    let len = self.eval_expr(&args[2]).to_num() as usize;
                    let effective_len = if start < 1 {
                        len.saturating_sub((1 - start) as usize)
                    } else {
                        len
                    };
                    let end = (start_idx + effective_len).min(chars.len());
                    AwkValue::Str(chars[start_idx..end].iter().collect())
                } else {
                    AwkValue::Str(chars[start_idx..].iter().collect())
                }
            }
            "index" => {
                if args.len() < 2 {
                    return AwkValue::Num(0.0);
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                let target = self.eval_expr(&args[1]).to_string_val();
                // Return character position, not byte position
                if target.is_empty() {
                    return AwkValue::Num(0.0);
                }
                let s_chars: Vec<char> = s.chars().collect();
                let t_chars: Vec<char> = target.chars().collect();
                let mut found = None;
                for i in 0..=s_chars.len().saturating_sub(t_chars.len()) {
                    if s_chars[i..i + t_chars.len()] == t_chars[..] {
                        found = Some(i);
                        break;
                    }
                }
                match found {
                    Some(pos) => AwkValue::Num((pos + 1) as f64),
                    None => AwkValue::Num(0.0),
                }
            }
            "split" => {
                if args.len() < 2 {
                    return AwkValue::Num(0.0);
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                let array_name = match &args[1] {
                    Expr::Var(name) => name.clone(),
                    _ => return AwkValue::Num(0.0),
                };
                let fs = if args.len() >= 3 {
                    self.eval_expr(&args[2]).to_string_val()
                } else {
                    self.get_var("FS").to_string_val()
                };
                let parts = split_fields(&s, &fs);
                // Clear existing array
                self.arrays.remove(&array_name);
                let arr = self.arrays.entry(array_name).or_default();
                for (i, part) in parts.iter().enumerate() {
                    arr.insert((i + 1).to_string(), AwkValue::Str(part.clone()));
                }
                AwkValue::Num(parts.len() as f64)
            }
            "sub" => self.func_sub(args, false),
            "gsub" => self.func_sub(args, true),
            "match" => {
                if args.len() < 2 {
                    return AwkValue::Num(0.0);
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                let pattern = match &args[1] {
                    Expr::Regex(r) => r.clone(),
                    _ => self.eval_expr(&args[1]).to_string_val(),
                };
                if let Ok(re) = self.get_regex(&pattern) {
                    if let Some(m) = re.find(&s) {
                        let rstart = (m.start() + 1) as f64;
                        let rlength = m.len() as f64;
                        self.variables
                            .insert("RSTART".to_string(), AwkValue::Num(rstart));
                        self.variables
                            .insert("RLENGTH".to_string(), AwkValue::Num(rlength));
                        AwkValue::Num(rstart)
                    } else {
                        self.variables
                            .insert("RSTART".to_string(), AwkValue::Num(0.0));
                        self.variables
                            .insert("RLENGTH".to_string(), AwkValue::Num(-1.0));
                        AwkValue::Num(0.0)
                    }
                } else {
                    AwkValue::Num(0.0)
                }
            }
            "sprintf" => {
                if args.is_empty() {
                    return AwkValue::Str(String::new());
                }
                let fmt = self.eval_expr(&args[0]).to_string_val();
                let vals: Vec<AwkValue> = args[1..].iter().map(|e| self.eval_expr(e)).collect();
                AwkValue::Str(awk_sprintf(&fmt, &vals))
            }
            "tolower" => {
                if args.is_empty() {
                    return AwkValue::Str(String::new());
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                AwkValue::Str(s.to_lowercase())
            }
            "toupper" => {
                if args.is_empty() {
                    return AwkValue::Str(String::new());
                }
                let s = self.eval_expr(&args[0]).to_string_val();
                AwkValue::Str(s.to_uppercase())
            }
            "int" => {
                if args.is_empty() {
                    return AwkValue::Num(0.0);
                }
                let n = self.eval_expr(&args[0]).to_num();
                AwkValue::Num(n.trunc())
            }
            "sqrt" => {
                let n = if args.is_empty() {
                    0.0
                } else {
                    self.eval_expr(&args[0]).to_num()
                };
                AwkValue::Num(n.sqrt())
            }
            "sin" => {
                let n = if args.is_empty() {
                    0.0
                } else {
                    self.eval_expr(&args[0]).to_num()
                };
                AwkValue::Num(n.sin())
            }
            "cos" => {
                let n = if args.is_empty() {
                    0.0
                } else {
                    self.eval_expr(&args[0]).to_num()
                };
                AwkValue::Num(n.cos())
            }
            "atan2" => {
                if args.len() < 2 {
                    return AwkValue::Num(0.0);
                }
                let y = self.eval_expr(&args[0]).to_num();
                let x = self.eval_expr(&args[1]).to_num();
                AwkValue::Num(y.atan2(x))
            }
            "exp" => {
                let n = if args.is_empty() {
                    0.0
                } else {
                    self.eval_expr(&args[0]).to_num()
                };
                AwkValue::Num(n.exp())
            }
            "log" => {
                let n = if args.is_empty() {
                    0.0
                } else {
                    self.eval_expr(&args[0]).to_num()
                };
                AwkValue::Num(n.ln())
            }
            "rand" => {
                // Simple LCG random number generator
                self.rng_state = self
                    .rng_state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                let val = (self.rng_state >> 33) as f64 / (1u64 << 31) as f64;
                AwkValue::Num(val)
            }
            "srand" => {
                let old_seed = self.rng_state;
                self.rng_state = if args.is_empty() {
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_nanos() as u64)
                        .unwrap_or(0)
                } else {
                    self.eval_expr(&args[0]).to_num() as u64
                };
                AwkValue::Num(old_seed as f64)
            }
            _ => {
                self.stderr
                    .push_str(&format!("awk: unknown function '{name}'\n"));
                AwkValue::Uninitialized
            }
        }
    }

    fn func_sub(&mut self, args: &[Expr], global: bool) -> AwkValue {
        if args.len() < 2 {
            return AwkValue::Num(0.0);
        }
        let pattern = match &args[0] {
            Expr::Regex(r) => r.clone(),
            _ => self.eval_expr(&args[0]).to_string_val(),
        };
        let replacement = self.eval_expr(&args[1]).to_string_val();

        // Target defaults to $0
        let (target_str, target_expr) = if args.len() >= 3 {
            let s = self.eval_expr(&args[2]).to_string_val();
            (s, Some(&args[2]))
        } else {
            (self.get_field(0), None)
        };

        let re = match self.get_regex(&pattern) {
            Ok(re) => re,
            Err(_) => return AwkValue::Num(0.0),
        };

        // Process replacement: & means matched text, \\ means literal backslash
        let mut count = 0;
        let mut result = String::new();
        let mut last_end = 0;

        for m in re.find_iter(&target_str) {
            result.push_str(&target_str[last_end..m.start()]);
            // Process replacement string
            let mut i = 0;
            let rep_bytes: Vec<char> = replacement.chars().collect();
            while i < rep_bytes.len() {
                if rep_bytes[i] == '&' {
                    result.push_str(m.as_str());
                } else if rep_bytes[i] == '\\' && i + 1 < rep_bytes.len() {
                    if rep_bytes[i + 1] == '&' {
                        result.push('&');
                        i += 1;
                    } else if rep_bytes[i + 1] == '\\' {
                        result.push('\\');
                        i += 1;
                    } else {
                        result.push(rep_bytes[i + 1]);
                        i += 1;
                    }
                } else {
                    result.push(rep_bytes[i]);
                }
                i += 1;
            }
            last_end = m.end();
            count += 1;
            if !global {
                break;
            }
        }
        result.push_str(&target_str[last_end..]);

        // Assign back to target
        let new_val = AwkValue::Str(result);
        if let Some(target) = target_expr {
            self.assign_to(target, new_val);
        } else {
            self.set_field(0, &new_val.to_string_val());
        }

        AwkValue::Num(count as f64)
    }

    // ── Regex helpers ────────────────────────────────────────────────

    fn regex_match(&mut self, pattern: &str, text: &str) -> bool {
        match self.get_regex(pattern) {
            Ok(re) => re.is_match(text),
            Err(_) => false,
        }
    }

    fn get_regex(&mut self, pattern: &str) -> Result<Regex, String> {
        if let Some(re) = self.regex_cache.get(pattern) {
            return Ok(re.clone());
        }
        match Regex::new(pattern) {
            Ok(re) => {
                if self.regex_cache.len() > 1000 {
                    self.regex_cache.clear();
                }
                self.regex_cache.insert(pattern.to_string(), re.clone());
                Ok(re)
            }
            Err(e) => {
                self.stderr
                    .push_str(&format!("awk: invalid regex '{pattern}': {e}\n"));
                Err(e.to_string())
            }
        }
    }
}

// ── Comparison helpers ──────────────────────────────────────────────────

fn compare_values(left: &AwkValue, right: &AwkValue, op: BinOp) -> bool {
    // If both look numeric, compare as numbers; otherwise compare as strings
    let use_numeric = is_numeric_value(left) && is_numeric_value(right);

    if use_numeric {
        let l = left.to_num();
        let r = right.to_num();
        match op {
            BinOp::Lt => l < r,
            BinOp::Le => l <= r,
            BinOp::Gt => l > r,
            BinOp::Ge => l >= r,
            BinOp::Eq => l == r,
            BinOp::Ne => l != r,
            _ => false,
        }
    } else {
        let l = left.to_string_val();
        let r = right.to_string_val();
        match op {
            BinOp::Lt => l < r,
            BinOp::Le => l <= r,
            BinOp::Gt => l > r,
            BinOp::Ge => l >= r,
            BinOp::Eq => l == r,
            BinOp::Ne => l != r,
            _ => false,
        }
    }
}

fn is_numeric_value(val: &AwkValue) -> bool {
    match val {
        AwkValue::Num(_) => true,
        AwkValue::Uninitialized => true,
        AwkValue::Str(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                return false;
            }
            trimmed.parse::<f64>().is_ok()
        }
    }
}

// ── Field splitting ─────────────────────────────────────────────────────

fn split_fields(record: &str, fs: &str) -> Vec<String> {
    if fs == " " {
        // Default FS: split on runs of whitespace, trim leading/trailing
        record.split_whitespace().map(|s| s.to_string()).collect()
    } else if fs.is_empty() {
        // Empty FS: split each character
        record.chars().map(|c| c.to_string()).collect()
    } else if fs.len() == 1 {
        // Single character FS
        record.split(fs).map(|s| s.to_string()).collect()
    } else {
        // Multi-char FS: treat as regex
        match Regex::new(fs) {
            Ok(re) => re.split(record).map(|s| s.to_string()).collect(),
            Err(_) => vec![record.to_string()],
        }
    }
}

fn split_records(input: &str, rs: &str) -> Vec<String> {
    if rs == "\n" {
        // Default RS: split on newlines, but don't include trailing empty record
        let mut records: Vec<String> = input.split('\n').map(|s| s.to_string()).collect();
        // Remove trailing empty record if input ends with newline
        if records.last().is_some_and(|s| s.is_empty()) {
            records.pop();
        }
        records
    } else if rs.is_empty() {
        // Empty RS: paragraph mode (split on blank lines)
        let mut records = Vec::new();
        let mut current = String::new();
        for line in input.split('\n') {
            if line.is_empty() {
                if !current.is_empty() {
                    // Remove trailing newline from current record
                    if current.ends_with('\n') {
                        current.pop();
                    }
                    records.push(current);
                    current = String::new();
                }
            } else {
                if !current.is_empty() {
                    current.push('\n');
                }
                current.push_str(line);
            }
        }
        if !current.is_empty() {
            records.push(current);
        }
        records
    } else if rs.len() == 1 {
        let mut records: Vec<String> = input.split(&rs[..1]).map(|s| s.to_string()).collect();
        if records.last().is_some_and(|s| s.is_empty()) {
            records.pop();
        }
        records
    } else {
        match Regex::new(rs) {
            Ok(re) => {
                let mut records: Vec<String> = re.split(input).map(|s| s.to_string()).collect();
                if records.last().is_some_and(|s| s.is_empty()) {
                    records.pop();
                }
                records
            }
            Err(_) => vec![input.to_string()],
        }
    }
}

// ── sprintf implementation ──────────────────────────────────────────────

fn awk_sprintf(fmt: &str, args: &[AwkValue]) -> String {
    let mut result = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    let mut arg_idx = 0;

    while i < chars.len() {
        if chars[i] == '%' {
            i += 1;
            if i >= chars.len() {
                result.push('%');
                break;
            }
            if chars[i] == '%' {
                result.push('%');
                i += 1;
                continue;
            }

            // Parse format specifier: [flags][width][.precision]type
            let mut flags = String::new();
            while i < chars.len() && "-+ #0".contains(chars[i]) {
                flags.push(chars[i]);
                i += 1;
            }
            let mut width = String::new();
            if i < chars.len() && chars[i] == '*' {
                // Width from argument
                if arg_idx < args.len() {
                    width = format!("{}", args[arg_idx].to_num() as i64);
                    arg_idx += 1;
                }
                i += 1;
            } else {
                while i < chars.len() && chars[i].is_ascii_digit() {
                    width.push(chars[i]);
                    i += 1;
                }
            }
            let mut precision = String::new();
            if i < chars.len() && chars[i] == '.' {
                i += 1;
                if i < chars.len() && chars[i] == '*' {
                    if arg_idx < args.len() {
                        precision = format!("{}", args[arg_idx].to_num() as i64);
                        arg_idx += 1;
                    }
                    i += 1;
                } else {
                    while i < chars.len() && chars[i].is_ascii_digit() {
                        precision.push(chars[i]);
                        i += 1;
                    }
                }
            }

            if i >= chars.len() {
                break;
            }

            let conv = chars[i];
            i += 1;

            let arg = if arg_idx < args.len() {
                let a = &args[arg_idx];
                arg_idx += 1;
                a.clone()
            } else {
                AwkValue::Uninitialized
            };

            let w: usize = width.parse().unwrap_or(0);
            let left_justify = flags.contains('-');
            let zero_pad = flags.contains('0') && !left_justify;

            let formatted = match conv {
                'd' | 'i' => {
                    let n = arg.to_num() as i64;
                    format!("{n}")
                }
                'o' => {
                    let n = arg.to_num() as i64;
                    format!("{n:o}")
                }
                'x' => {
                    let n = arg.to_num() as i64;
                    format!("{n:x}")
                }
                'X' => {
                    let n = arg.to_num() as i64;
                    format!("{n:X}")
                }
                'f' => {
                    let n = arg.to_num();
                    let prec: usize = if precision.is_empty() {
                        6
                    } else {
                        precision.parse().unwrap_or(6)
                    };
                    format!("{n:.prec$}")
                }
                'e' => {
                    let n = arg.to_num();
                    let prec: usize = if precision.is_empty() {
                        6
                    } else {
                        precision.parse().unwrap_or(6)
                    };
                    format_scientific(n, prec, false)
                }
                'E' => {
                    let n = arg.to_num();
                    let prec: usize = if precision.is_empty() {
                        6
                    } else {
                        precision.parse().unwrap_or(6)
                    };
                    format_scientific(n, prec, true)
                }
                'g' | 'G' => {
                    let n = arg.to_num();
                    let prec: usize = if precision.is_empty() {
                        6
                    } else {
                        precision.parse().unwrap_or(6)
                    };
                    format_g(n, prec)
                }
                's' => {
                    let mut s = arg.to_string_val();
                    if !precision.is_empty() {
                        let prec: usize = precision.parse().unwrap_or(s.len());
                        if s.len() > prec {
                            s.truncate(prec);
                        }
                    }
                    s
                }
                'c' => match &arg {
                    AwkValue::Str(s) if !s.is_empty() => s.chars().next().unwrap().to_string(),
                    _ => {
                        let n = arg.to_num() as u32;
                        char::from_u32(n).map(|c| c.to_string()).unwrap_or_default()
                    }
                },
                _ => {
                    // Unknown format specifier, output as-is
                    format!("%{flags}{width}{conv}")
                }
            };

            // Apply width and padding
            if w > 0 && formatted.len() < w {
                let padding = w - formatted.len();
                if left_justify {
                    result.push_str(&formatted);
                    for _ in 0..padding {
                        result.push(' ');
                    }
                } else if zero_pad && matches!(conv, 'd' | 'i' | 'f' | 'e' | 'E' | 'g' | 'G') {
                    // Zero-pad numbers
                    if let Some(rest) = formatted.strip_prefix('-') {
                        result.push('-');
                        for _ in 0..padding {
                            result.push('0');
                        }
                        result.push_str(rest);
                    } else {
                        for _ in 0..padding {
                            result.push('0');
                        }
                        result.push_str(&formatted);
                    }
                } else {
                    for _ in 0..padding {
                        result.push(' ');
                    }
                    result.push_str(&formatted);
                }
            } else {
                result.push_str(&formatted);
            }
        } else if chars[i] == '\\' {
            i += 1;
            if i < chars.len() {
                match chars[i] {
                    'n' => result.push('\n'),
                    't' => result.push('\t'),
                    'r' => result.push('\r'),
                    '\\' => result.push('\\'),
                    '"' => result.push('"'),
                    'a' => result.push('\x07'),
                    'b' => result.push('\x08'),
                    'f' => result.push('\x0c'),
                    '/' => result.push('/'),
                    _ => {
                        result.push('\\');
                        result.push(chars[i]);
                    }
                }
                i += 1;
            } else {
                result.push('\\');
            }
        } else {
            result.push(chars[i]);
            i += 1;
        }
    }

    result
}

fn format_scientific(n: f64, prec: usize, upper: bool) -> String {
    if n == 0.0 {
        let e_char = if upper { 'E' } else { 'e' };
        return format!("{:.prec$}{e_char}+00", 0.0);
    }
    let exp = n.abs().log10().floor() as i32;
    let mantissa = n / 10f64.powi(exp);
    let e_char = if upper { 'E' } else { 'e' };
    format!("{mantissa:.prec$}{e_char}{exp:+03}")
}

fn format_g(n: f64, prec: usize) -> String {
    if n == 0.0 {
        return "0".to_string();
    }
    let prec = if prec == 0 { 1 } else { prec };
    let exp = if n != 0.0 {
        n.abs().log10().floor() as i32
    } else {
        0
    };
    if exp >= -4 && exp < prec as i32 {
        // Use fixed notation
        let decimal_digits = (prec as i32 - 1 - exp).max(0) as usize;
        let s = format!("{n:.decimal_digits$}");
        // Remove trailing zeros after decimal point
        trim_trailing_zeros(&s)
    } else {
        // Use scientific notation
        format_scientific(n, prec - 1, false)
    }
}

fn trim_trailing_zeros(s: &str) -> String {
    if !s.contains('.') {
        return s.to_string();
    }
    let trimmed = s.trim_end_matches('0');
    if let Some(without_dot) = trimmed.strip_suffix('.') {
        without_dot.to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::awk::lexer::Lexer;
    use crate::commands::awk::parser::Parser;

    fn run_awk(program: &str, input: &str) -> String {
        let tokens = Lexer::new(program).tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        let inputs = if input.is_empty() {
            vec![]
        } else {
            vec![("".to_string(), input.to_string())]
        };
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        stdout
    }

    fn run_awk_full(program: &str, input: &str) -> (i32, String, String) {
        let tokens = Lexer::new(program).tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        let inputs = if input.is_empty() {
            vec![]
        } else {
            vec![("".to_string(), input.to_string())]
        };
        runtime.execute(&ast, &inputs)
    }

    #[test]
    fn print_first_field() {
        assert_eq!(
            run_awk("{print $1}", "hello world\nfoo bar\n"),
            "hello\nfoo\n"
        );
    }

    #[test]
    fn print_all_fields() {
        assert_eq!(run_awk("{print $0}", "hello world\n"), "hello world\n");
    }

    #[test]
    fn custom_field_separator() {
        let tokens = Lexer::new("{print $1}").tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        runtime.set_var("FS", ":");
        let inputs = vec![("".to_string(), "root:x:0:0\n".to_string())];
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        assert_eq!(stdout, "root\n");
    }

    #[test]
    fn field_assignment_rebuilds_record() {
        assert_eq!(run_awk("{$2 = \"X\"; print $0}", "a b c\n"), "a X c\n");
    }

    #[test]
    fn regex_pattern() {
        assert_eq!(
            run_awk("/error/ {print}", "info: ok\nerror: fail\ninfo: done\n"),
            "error: fail\n"
        );
    }

    #[test]
    fn begin_end_sum() {
        assert_eq!(
            run_awk("BEGIN{sum=0} {sum+=$1} END{print sum}", "10\n20\n30\n"),
            "60\n"
        );
    }

    #[test]
    fn variable_flag() {
        let tokens = Lexer::new("$1 > threshold").tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        runtime.set_var("threshold", "10");
        let inputs = vec![("".to_string(), "5\n15\n8\n20\n".to_string())];
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        assert_eq!(stdout, "15\n20\n");
    }

    #[test]
    fn uninitialized_variables() {
        assert_eq!(run_awk("{print x+0, x}", "line\n"), "0 \n");
    }

    #[test]
    fn arithmetic_in_output() {
        assert_eq!(run_awk("{print $1, $1*2}", "5\n10\n"), "5 10\n10 20\n");
    }

    #[test]
    fn if_else_statement() {
        assert_eq!(
            run_awk(
                "{if ($1 > 10) print \"big\"; else print \"small\"}",
                "5\n15\n"
            ),
            "small\nbig\n"
        );
    }

    #[test]
    fn printf_formatting() {
        assert_eq!(
            run_awk("{printf \"%-10s %5d\\n\", $1, $2}", "hello 42\n"),
            "hello         42\n"
        );
    }

    #[test]
    fn array_word_count() {
        let output = run_awk(
            "{count[$1]++} END{for(k in count) print k, count[k]}",
            "a\nb\na\nc\nb\na\n",
        );
        // Order may vary; check all entries exist
        assert!(output.contains("a 3"));
        assert!(output.contains("b 2"));
        assert!(output.contains("c 1"));
    }

    #[test]
    fn toupper_function() {
        assert_eq!(run_awk("{print toupper($0)}", "hello\n"), "HELLO\n");
    }

    #[test]
    fn split_function() {
        assert_eq!(
            run_awk(
                "{n=split($0, a, \":\"); for(i=1;i<=n;i++) print a[i]}",
                "a:b:c\n"
            ),
            "a\nb\nc\n"
        );
    }

    #[test]
    fn sub_function() {
        assert_eq!(
            run_awk("{sub(/world/, \"earth\"); print}", "hello world\n"),
            "hello earth\n"
        );
    }

    #[test]
    fn gsub_function() {
        assert_eq!(run_awk("{gsub(/o/, \"0\"); print}", "foobar\n"), "f00bar\n");
    }

    #[test]
    fn no_action_implicit_print() {
        assert_eq!(
            run_awk("/hello/", "hello world\ngoodbye\nhello again\n"),
            "hello world\nhello again\n"
        );
    }

    #[test]
    fn empty_input() {
        assert_eq!(run_awk("{print}", ""), "");
    }

    #[test]
    fn nr_and_nf() {
        assert_eq!(run_awk("{print NR, NF}", "a b c\nx y\n"), "1 3\n2 2\n");
    }

    #[test]
    fn ternary_expression() {
        assert_eq!(
            run_awk("{print ($1 > 0) ? \"pos\" : \"neg\"}", "5\n-3\n"),
            "pos\nneg\n"
        );
    }

    #[test]
    fn delete_array_element() {
        let output = run_awk(
            "{a[$1]=1} END{delete a[\"b\"]; for(k in a) print k}",
            "a\nb\nc\n",
        );
        assert!(output.contains('a'));
        assert!(output.contains('c'));
        assert!(!output.contains('b'));
    }

    #[test]
    fn while_loop() {
        assert_eq!(
            run_awk(
                "BEGIN{i=1; while(i<=5){printf \"%d \",i; i++}; print \"\"}",
                ""
            ),
            "1 2 3 4 5 \n"
        );
    }

    #[test]
    fn for_loop() {
        assert_eq!(
            run_awk("BEGIN{for(i=1;i<=3;i++) printf \"%d \",i; print \"\"}", ""),
            "1 2 3 \n"
        );
    }

    #[test]
    fn next_statement() {
        assert_eq!(
            run_awk(
                "{if ($1 == \"skip\") next; print}",
                "keep\nskip\nalso keep\n"
            ),
            "keep\nalso keep\n"
        );
    }

    #[test]
    fn exit_statement() {
        let (code, stdout, _) = run_awk_full("{ if (NR==2) exit 42; print }", "a\nb\nc\n");
        assert_eq!(stdout, "a\n");
        assert_eq!(code, 42);
    }

    #[test]
    fn range_pattern() {
        assert_eq!(
            run_awk(
                "/start/,/end/ {print}",
                "before\nstart here\nmiddle\nend here\nafter\n"
            ),
            "start here\nmiddle\nend here\n"
        );
    }

    #[test]
    fn multi_file_fnr_nr() {
        let tokens = Lexer::new("{print FILENAME, FNR, NR}").tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        let inputs = vec![
            ("file1".to_string(), "a\nb\n".to_string()),
            ("file2".to_string(), "c\n".to_string()),
        ];
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        assert_eq!(stdout, "file1 1 1\nfile1 2 2\nfile2 1 3\n");
    }

    #[test]
    fn empty_fs_splits_chars() {
        let tokens = Lexer::new("{print $1, $2, $3}").tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        runtime.set_var("FS", "");
        let inputs = vec![("".to_string(), "abc\n".to_string())];
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        assert_eq!(stdout, "a b c\n");
    }

    #[test]
    fn match_function() {
        assert_eq!(
            run_awk(
                "{if (match($0, /[0-9]+/)) print RSTART, RLENGTH}",
                "abc123def\n"
            ),
            "4 3\n"
        );
    }

    #[test]
    fn index_function() {
        assert_eq!(
            run_awk("{print index($0, \"world\")}", "hello world\n"),
            "7\n"
        );
    }

    #[test]
    fn substr_function() {
        assert_eq!(run_awk("{print substr($0, 7)}", "hello world\n"), "world\n");
    }

    #[test]
    fn sprintf_function() {
        assert_eq!(run_awk("{print sprintf(\"%05d\", $1)}", "42\n"), "00042\n");
    }

    #[test]
    fn in_array_test() {
        assert_eq!(
            run_awk("{a[$1]=1} END{print (\"x\" in a), (\"z\" in a)}", "x\ny\n"),
            "1 0\n"
        );
    }

    #[test]
    fn assignment_operators() {
        assert_eq!(run_awk("BEGIN{x=10; x+=5; x-=3; print x}", ""), "12\n");
    }

    #[test]
    fn do_while_loop() {
        assert_eq!(
            run_awk(
                "BEGIN{i=1; do { printf \"%d \", i; i++ } while(i<=3); print \"\"}",
                ""
            ),
            "1 2 3 \n"
        );
    }

    #[test]
    fn string_comparison() {
        assert_eq!(
            run_awk("{if ($1 == \"hello\") print \"match\"}", "hello\nworld\n"),
            "match\n"
        );
    }

    #[test]
    fn regex_match_operator() {
        assert_eq!(
            run_awk("{if ($0 ~ /^[0-9]/) print}", "123\nabc\n456\n"),
            "123\n456\n"
        );
    }

    #[test]
    fn regex_not_match_operator() {
        assert_eq!(
            run_awk("{if ($0 !~ /^[0-9]/) print}", "123\nabc\n456\n"),
            "abc\n"
        );
    }

    #[test]
    fn power_operator() {
        assert_eq!(run_awk("BEGIN{print 2^10}", ""), "1024\n");
    }

    #[test]
    fn int_function() {
        assert_eq!(run_awk("BEGIN{print int(3.9)}", ""), "3\n");
    }

    #[test]
    fn length_of_array() {
        assert_eq!(
            run_awk("{a[$1]=1} END{print length(a)}", "x\ny\nz\n"),
            "3\n"
        );
    }

    #[test]
    fn break_in_loop() {
        assert_eq!(
            run_awk(
                "BEGIN{for(i=1;i<=10;i++){if(i==4) break; printf \"%d \",i}; print \"\"}",
                ""
            ),
            "1 2 3 \n"
        );
    }

    #[test]
    fn continue_in_loop() {
        assert_eq!(
            run_awk(
                "BEGIN{for(i=1;i<=5;i++){if(i==3) continue; printf \"%d \",i}; print \"\"}",
                ""
            ),
            "1 2 4 5 \n"
        );
    }

    #[test]
    fn multi_dim_array_subsep() {
        let output = run_awk(
            "BEGIN{a[1,2]=\"x\"; a[3,4]=\"y\"; for(k in a) print k, a[k]}",
            "",
        );
        assert!(output.contains("1\x1c2 x"));
        assert!(output.contains("3\x1c4 y"));
    }

    #[test]
    fn implicit_concatenation() {
        assert_eq!(
            run_awk("BEGIN{x = \"hello\" \" \" \"world\"; print x}", ""),
            "hello world\n"
        );
    }

    #[test]
    fn print_with_ofs() {
        let tokens = Lexer::new("{print $1, $2}").tokenize().unwrap();
        let ast = Parser::new(tokens).parse().unwrap();
        let mut runtime = AwkRuntime::new();
        runtime.set_var("OFS", "-");
        let inputs = vec![("".to_string(), "a b\n".to_string())];
        let (_, stdout, _) = runtime.execute(&ast, &inputs);
        assert_eq!(stdout, "a-b\n");
    }

    #[test]
    fn logical_operators() {
        assert_eq!(run_awk("{print ($1 > 0 && $1 < 10)}", "5\n15\n"), "1\n0\n");
    }

    #[test]
    fn unary_not() {
        assert_eq!(run_awk("{print !($1 > 10)}", "5\n15\n"), "1\n0\n");
    }

    #[test]
    fn modulo_operator() {
        assert_eq!(run_awk("{print $1 % 3}", "10\n7\n"), "1\n1\n");
    }

    #[test]
    fn delete_entire_array() {
        assert_eq!(
            run_awk("{a[$1]=1} END{delete a; print length(a)}", "x\ny\n"),
            "0\n"
        );
    }

    #[test]
    fn tolower_function() {
        assert_eq!(run_awk("{print tolower($0)}", "HELLO\n"), "hello\n");
    }

    #[test]
    fn single_field_record() {
        assert_eq!(run_awk("{print $1, NF}", "hello\n"), "hello 1\n");
    }

    #[test]
    fn expression_pattern() {
        assert_eq!(
            run_awk("NR > 1 {print}", "skip\nkeep1\nkeep2\n"),
            "keep1\nkeep2\n"
        );
    }
}
