/*
Copyright 2026 Google LLC

Licensed under the Apache License, Version 2.0 (the "License");
you may not use this file except in compliance with the License.
You may obtain a copy of the License at

     https://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software
distributed under the License is distributed on an "AS IS" BASIS,
WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
See the License for the specific language governing permissions and
limitations under the License.
*/

//! 轻量的 textproto 风格 quirks 规则解析，对应主线 C++
//! `sommelier-quirks.cc` / `quirks.proto`（不引入 protobuf crate）。
//!
//! 规则按定义顺序保存；**后定义的规则优先级更高**。语义与主线
//! `Quirks::Update()` 一致：
//! * `steam_game_id` 规则写入 per-game 表；
//! * `always` 规则写入 always 表，并清掉**此前**为该 feature 定义的
//!   所有 per-game 条目；
//! * 查询时 per-game 命中优先于 always，但之后定义的 per-game 规则会
//!   重新覆盖 always。

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Feature {
    X11MoveWindows,
    BlackScreenFix,
    ContainerizeWindows,
    ForceX11Unmaximize,
    RandrEmuFix,
}

impl Feature {
    pub const ALL: &'static [Feature] = &[
        Feature::X11MoveWindows,
        Feature::BlackScreenFix,
        Feature::ContainerizeWindows,
        Feature::ForceX11Unmaximize,
        Feature::RandrEmuFix,
    ];

    /// 解析枚举名。支持 `FEATURE_X11_MOVE_WINDOWS`、无前缀
    /// `X11_MOVE_WINDOWS`、大小写混合、连字符以及 Rust 风格驼峰。
    pub fn parse(s: &str) -> Option<Self> {
        let upper = s.trim().to_ascii_uppercase().replace('-', "_");
        let n = upper.strip_prefix("FEATURE_").unwrap_or(upper.as_str());
        // 去掉下划线后比较，宽容驼峰/下划线混用。
        let compact: String = n.chars().filter(|c| *c != '_').collect();
        match compact.as_str() {
            "X11MOVEWINDOWS" => Some(Feature::X11MoveWindows),
            "BLACKSCREENFIX" => Some(Feature::BlackScreenFix),
            "CONTAINERIZEWINDOWS" => Some(Feature::ContainerizeWindows),
            "FORCEX11UNMAXIMIZE" => Some(Feature::ForceX11Unmaximize),
            "RANDREMUFIX" => Some(Feature::RandrEmuFix),
            _ => None,
        }
    }

    /// proto 枚举名（与主线 `quirks::Feature_Name()` 输出一致）。
    pub fn name(self) -> &'static str {
        match self {
            Feature::X11MoveWindows => "FEATURE_X11_MOVE_WINDOWS",
            Feature::BlackScreenFix => "FEATURE_BLACK_SCREEN_FIX",
            Feature::ContainerizeWindows => "FEATURE_CONTAINERIZE_WINDOWS",
            Feature::ForceX11Unmaximize => "FEATURE_FORCE_X11_UNMAXIMIZE",
            Feature::RandrEmuFix => "FEATURE_RANDR_EMU_FIX",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Condition {
    SteamGameId(u32),
    Always,
}

#[derive(Debug, Clone)]
struct Rule {
    conditions: Vec<Condition>,
    enable: Vec<Feature>,
    disable: Vec<Feature>,
}

#[derive(Debug, Clone, Default)]
pub struct Quirks {
    /// 原始规则，保留定义顺序（merge 时追加）。
    rules: Vec<Rule>,
    /// (steam_game_id, feature) -> enabled，主线 feature_to_steam_id_。
    per_game: HashMap<(u32, Feature), bool>,
    /// feature -> enabled，主线 always_features_。
    always: HashMap<Feature, bool>,
}

impl Quirks {
    pub fn new() -> Self {
        Self::default()
    }

    /// 解析 textproto 风格文本。支持 `#` / `//` 注释、任意空白、可选冒号、
    /// 可选引号、逗号/分号分隔符；语法或未知枚举错误带行号。
    pub fn parse(text: &str) -> Result<Self, String> {
        let tokens = tokenize(text)?;
        let mut parser = Parser { tokens, pos: 0 };
        let rules = parser.parse_config()?;
        let mut quirks = Quirks {
            rules,
            ..Self::default()
        };
        quirks.rebuild();
        Ok(quirks)
    }

    /// 主线 `LoadFromCommaSeparatedFiles`：按逗号分割路径依次加载并 merge。
    /// 单个文件打不开时记录 warn 并跳过（主线 LOG(ERROR) 后继续），
    /// 内容解析错误则返回带路径与行号的 Err。
    pub fn load_comma_separated_files(paths: &str) -> Result<Self, String> {
        let mut out = Self::new();
        for raw in paths.split(',') {
            let path = raw.trim();
            if path.is_empty() {
                continue;
            }
            let content = match std::fs::read_to_string(path) {
                Ok(c) => c,
                Err(e) => {
                    log::warn!("failed to open quirks config {}: {}", path, e);
                    continue;
                }
            };
            let parsed = Self::parse(&content).map_err(|e| format!("{}: {}", path, e))?;
            out.merge(parsed);
        }
        Ok(out)
    }

    /// 追加 `other` 的规则（后定义者优先级更高），然后重建匹配表。
    pub fn merge(&mut self, other: Quirks) {
        self.rules.extend(other.rules);
        self.rebuild();
    }

    /// 主线 `Quirks::Update()`：按定义顺序重放规则，生成查询表。
    fn rebuild(&mut self) {
        self.per_game.clear();
        self.always.clear();
        for rule in &self.rules {
            // 主线只支持单 condition，其余规则整条跳过。
            if rule.conditions.len() != 1 {
                continue;
            }
            match rule.conditions[0] {
                Condition::SteamGameId(id) => {
                    for &f in &rule.enable {
                        self.per_game.insert((id, f), true);
                    }
                    for &f in &rule.disable {
                        self.per_game.insert((id, f), false);
                    }
                }
                Condition::Always => {
                    for &f in &rule.enable {
                        self.always.insert(f, true);
                        // A global rule resets any earlier per-game override.
                        // Borrow the field directly (not `&mut self`) so it is
                        // disjoint from the immutable `&self.rules` above.
                        clear_per_game(&mut self.per_game, f);
                    }
                    for &f in &rule.disable {
                        self.always.insert(f, false);
                        clear_per_game(&mut self.per_game, f);
                    }
                }
            }
        }
    }

    /// 主线 `Quirks::IsEnabled`：per-game 命中优先于 always。
    pub fn is_enabled(&self, steam_game_id: u32, feature: Feature) -> bool {
        match self.per_game.get(&(steam_game_id, feature)) {
            Some(&enabled) => enabled,
            None => self.always.get(&feature).copied().unwrap_or(false),
        }
    }

    /// 按 `Feature::ALL`（proto 枚举数值顺序）返回启用的 feature。
    pub fn enabled_features(&self, steam_game_id: u32) -> Vec<Feature> {
        Feature::ALL
            .iter()
            .copied()
            .filter(|&f| self.is_enabled(steam_game_id, f))
            .collect()
    }

    /// `--print-enabled-features`：每行一个 proto 枚举名（主线
    /// `PrintFeaturesEnabled` 用 `printf("%s\n")`），不含末尾换行。
    pub fn print_features(&self, steam_game_id: u32) -> String {
        self.enabled_features(steam_game_id)
            .iter()
            .map(|f| f.name())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Remove all per-game overrides for `feature`. Kept as a free function (not a
/// `&mut self` method) so `rebuild()` can iterate `&self.rules` while mutating
/// `self.per_game`: Rust permits disjoint field borrows, but not `&mut self`
/// while another field is borrowed.
fn clear_per_game(per_game: &mut HashMap<(u32, Feature), bool>, feature: Feature) {
    per_game.retain(|key, _| key.1 != feature);
}

// ---------------------------------------------------------------------------
// Tokenizer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum TokKind {
    Word(String),
    LBrace,
    RBrace,
    Colon,
}

#[derive(Debug, Clone)]
struct Tok {
    kind: TokKind,
    line: usize,
}

fn tokenize(text: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0usize;
    let mut line = 1usize;

    while i < chars.len() {
        let c = chars[i];
        if c == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '#' || (c == '/' && i + 1 < chars.len() && chars[i + 1] == '/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        match c {
            '{' => {
                out.push(Tok {
                    kind: TokKind::LBrace,
                    line,
                });
                i += 1;
            }
            '}' => {
                out.push(Tok {
                    kind: TokKind::RBrace,
                    line,
                });
                i += 1;
            }
            ':' => {
                out.push(Tok {
                    kind: TokKind::Colon,
                    line,
                });
                i += 1;
            }
            // textproto 允许用逗号/分号分隔字段，忽略即可。
            ',' | ';' => {
                i += 1;
            }
            '"' | '\'' => {
                let quote = c;
                let start_line = line;
                i += 1;
                let mut word = String::new();
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    word.push(chars[i]);
                    i += 1;
                }
                if i >= chars.len() {
                    return Err(format!("line {}: unterminated string", start_line));
                }
                i += 1;
                out.push(Tok {
                    kind: TokKind::Word(word),
                    line: start_line,
                });
            }
            _ => {
                let start = i;
                let start_line = line;
                while i < chars.len() {
                    let d = chars[i];
                    if d.is_whitespace()
                        || matches!(d, '{' | '}' | ':' | ',' | ';' | '#')
                        || (d == '/' && i + 1 < chars.len() && chars[i + 1] == '/')
                    {
                        break;
                    }
                    i += 1;
                }
                let word: String = chars[start..i].iter().copied().collect();
                out.push(Tok {
                    kind: TokKind::Word(word),
                    line: start_line,
                });
            }
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek_kind(&self) -> Option<TokKind> {
        self.tokens.get(self.pos).map(|t| t.kind.clone())
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        if t.is_some() {
            self.pos += 1;
        }
        t
    }

    fn current_line(&self) -> usize {
        self.tokens
            .get(self.pos)
            .or_else(|| self.tokens.last())
            .map(|t| t.line)
            .unwrap_or(1)
    }

    fn err(&self, msg: &str) -> String {
        format!("line {}: {}", self.current_line(), msg)
    }

    fn expect_word(&mut self) -> Result<(String, usize), String> {
        match self.next() {
            Some(Tok {
                kind: TokKind::Word(w),
                line,
            }) => Ok((w, line)),
            Some(t) => Err(format!(
                "line {}: expected a value, found {:?}",
                t.line, t.kind
            )),
            None => Err(self.err("expected a value, found end of input")),
        }
    }

    fn expect_lbrace(&mut self) -> Result<(), String> {
        match self.next() {
            Some(Tok {
                kind: TokKind::LBrace,
                ..
            }) => Ok(()),
            Some(t) => Err(format!("line {}: expected '{{'", t.line)),
            None => Err(self.err("expected '{', found end of input")),
        }
    }

    fn skip_optional_colon(&mut self) {
        if matches!(self.peek_kind(), Some(TokKind::Colon)) {
            self.pos += 1;
        }
    }

    /// 跳过一个未知字段的值（冒号后的单词，或 `{ ... }` 子块）。
    /// 不主动消费紧跟的 Word，避免误吃掉后续规则的关键字。
    fn skip_item_value(&mut self) -> Result<(), String> {
        self.skip_optional_colon();
        match self.peek_kind() {
            Some(TokKind::LBrace) => {
                self.pos += 1;
                self.skip_balanced()
            }
            Some(TokKind::RBrace) | Some(TokKind::Colon) | Some(TokKind::Word(_)) | None => Ok(()),
        }
    }

    /// 已消费一个 `{`，跳到与之匹配的 `}`（支持嵌套）。
    fn skip_balanced(&mut self) -> Result<(), String> {
        let mut depth = 1usize;
        while let Some(t) = self.next() {
            match t.kind {
                TokKind::LBrace => depth += 1,
                TokKind::RBrace => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(());
                    }
                }
                _ => {}
            }
        }
        Err(self.err("unterminated block"))
    }

    fn parse_config(&mut self) -> Result<Vec<Rule>, String> {
        let mut rules = Vec::new();
        while let Some(tok) = self.next() {
            let line = tok.line;
            match tok.kind {
                TokKind::Word(ref w) if w.eq_ignore_ascii_case("sommelier") => {
                    rules.push(self.parse_rule()?);
                }
                TokKind::RBrace => {
                    return Err(format!("line {}: unexpected '}}'", line));
                }
                // 宽容：跳过未知顶层字段/块。
                _ => self.skip_item_value()?,
            }
        }
        Ok(rules)
    }

    fn parse_rule(&mut self) -> Result<Rule, String> {
        self.skip_optional_colon();
        self.expect_lbrace()?;
        let mut rule = Rule {
            conditions: Vec::new(),
            enable: Vec::new(),
            disable: Vec::new(),
        };
        loop {
            match self.peek_kind() {
                Some(TokKind::RBrace) => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.err("unterminated 'sommelier' block")),
                _ => {}
            }
            let (name, _) = self.expect_word()?;
            match name.to_ascii_lowercase().as_str() {
                "condition" => {
                    self.skip_optional_colon();
                    self.expect_lbrace()?;
                    rule.conditions.extend(self.parse_condition_body()?);
                }
                "enable" | "disable" => {
                    self.skip_optional_colon();
                    let (value, value_line) = self.expect_word()?;
                    let feature = Feature::parse(&value).ok_or_else(|| {
                        format!("line {}: unknown feature '{}'", value_line, value)
                    })?;
                    if name.eq_ignore_ascii_case("enable") {
                        rule.enable.push(feature);
                    } else {
                        rule.disable.push(feature);
                    }
                }
                _ => self.skip_item_value()?,
            }
        }
        Ok(rule)
    }

    fn parse_condition_body(&mut self) -> Result<Vec<Condition>, String> {
        let mut conditions = Vec::new();
        loop {
            match self.peek_kind() {
                Some(TokKind::RBrace) => {
                    self.pos += 1;
                    break;
                }
                None => return Err(self.err("unterminated 'condition' block")),
                _ => {}
            }
            let (name, _) = self.expect_word()?;
            match name.to_ascii_lowercase().as_str() {
                "steam_game_id" => {
                    self.skip_optional_colon();
                    let (value, value_line) = self.expect_word()?;
                    let id = value.parse::<u32>().map_err(|_| {
                        format!("line {}: invalid steam_game_id '{}'", value_line, value)
                    })?;
                    conditions.push(Condition::SteamGameId(id));
                }
                "always" => {
                    self.skip_optional_colon();
                    let (value, value_line) = self.expect_word()?;
                    match parse_bool(&value) {
                        Some(true) => conditions.push(Condition::Always),
                        // 主线 `always: false` 不匹配任何条件，整条规则失效。
                        Some(false) => {}
                        None => {
                            return Err(format!(
                                "line {}: invalid always value '{}'",
                                value_line, value
                            ));
                        }
                    }
                }
                _ => self.skip_item_value()?,
            }
        }
        Ok(conditions)
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "true" | "1" => Some(true),
        "false" | "0" => Some(false),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// 对主线语义做的简化（详见最终报告）
// ---------------------------------------------------------------------------
// * 不引入 protobuf：只解析 `sommelier` / `condition` / `enable` / `disable`
//   及 `steam_game_id` / `always` 字段，其余字段宽容跳过。
// * `condition` 支持多实例但只接受恰好 1 个（与主线 Update() 一致）；
//   不支持 proto3 oneof 之外的条件类型，也不做隐式 AND 求值。
// * 不支持 `FEATURE_UNSPECIFIED`（不在公开 Feature 枚举内），遇到即报错。
// * 文件打不开时 warn 并跳过（主线 LOG(ERROR) 后继续），解析错误仍返回 Err。
// * 不匹配 window 对象，不写 X11 property / 不做 quirk 日志。

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic_rules() {
        let text = r#"
# per-game + default rules
sommelier {
  condition { steam_game_id: 123 }
  enable: FEATURE_X11_MOVE_WINDOWS
  enable: FEATURE_BLACK_SCREEN_FIX
}
sommelier {
  condition { always: true }
  enable: FEATURE_CONTAINERIZE_WINDOWS
}
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(q.is_enabled(123, Feature::X11MoveWindows));
        assert!(q.is_enabled(123, Feature::BlackScreenFix));
        assert!(q.is_enabled(123, Feature::ContainerizeWindows));
        assert!(!q.is_enabled(456, Feature::X11MoveWindows));
        assert!(!q.is_enabled(456, Feature::BlackScreenFix));
        assert!(q.is_enabled(456, Feature::ContainerizeWindows));
        assert!(!q.is_enabled(123, Feature::RandrEmuFix));
    }

    #[test]
    fn steam_rule_defined_after_always_takes_priority() {
        let text = r#"
sommelier { condition { always: true } enable: FEATURE_X11_MOVE_WINDOWS }
sommelier { condition { steam_game_id: 7 } disable: FEATURE_X11_MOVE_WINDOWS }
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(!q.is_enabled(7, Feature::X11MoveWindows));
        assert!(q.is_enabled(8, Feature::X11MoveWindows));
    }

    #[test]
    fn later_always_clears_earlier_per_game_definitions() {
        // 用户示例：先按 game enable，后出现 always disable，清掉此前 per-game。
        let text = r#"
sommelier { condition { steam_game_id: 123 } enable: FEATURE_X11_MOVE_WINDOWS }
sommelier { condition { always: true } disable: FEATURE_X11_MOVE_WINDOWS }
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(!q.is_enabled(123, Feature::X11MoveWindows));
        assert!(!q.is_enabled(999, Feature::X11MoveWindows));
    }

    #[test]
    fn merge_appends_rules_in_order() {
        let mut a = Quirks::parse(
            "sommelier { condition { always: true } enable: FEATURE_X11_MOVE_WINDOWS }",
        )
        .unwrap();
        let b = Quirks::parse(
            "sommelier { condition { always: true } disable: FEATURE_X11_MOVE_WINDOWS }",
        )
        .unwrap();
        a.merge(b);
        assert!(!a.is_enabled(1, Feature::X11MoveWindows));

        // 反向 merge：后追加的 enable 生效。
        let mut c = Quirks::parse(
            "sommelier { condition { always: true } disable: FEATURE_BLACK_SCREEN_FIX }",
        )
        .unwrap();
        c.merge(
            Quirks::parse(
                "sommelier { condition { steam_game_id: 5 } enable: FEATURE_BLACK_SCREEN_FIX }",
            )
            .unwrap(),
        );
        assert!(c.is_enabled(5, Feature::BlackScreenFix));
        assert!(!c.is_enabled(6, Feature::BlackScreenFix));
    }

    #[test]
    fn feature_parse_accepts_prefix_and_bare_forms() {
        assert_eq!(
            Feature::parse("FEATURE_X11_MOVE_WINDOWS"),
            Some(Feature::X11MoveWindows)
        );
        assert_eq!(
            Feature::parse("X11_MOVE_WINDOWS"),
            Some(Feature::X11MoveWindows)
        );
        assert_eq!(
            Feature::parse("x11-move-windows"),
            Some(Feature::X11MoveWindows)
        );
        assert_eq!(
            Feature::parse("X11MoveWindows"),
            Some(Feature::X11MoveWindows)
        );
        assert_eq!(
            Feature::parse("force_x11_unmaximize"),
            Some(Feature::ForceX11Unmaximize)
        );
        assert_eq!(
            Feature::parse("FEATURE_RANDR_EMU_FIX"),
            Some(Feature::RandrEmuFix)
        );
        assert_eq!(Feature::parse("FEATURE_BOGUS"), None);
        assert_eq!(Feature::X11MoveWindows.name(), "FEATURE_X11_MOVE_WINDOWS");
        assert_eq!(Feature::ALL.len(), 5);
    }

    #[test]
    fn unknown_feature_error_includes_line_number() {
        let text = "sommelier {\n  condition { always: true }\n  enable: FEATURE_NOPE\n}";
        let err = Quirks::parse(text).unwrap_err();
        assert!(err.contains("line 3"), "{err}");
        assert!(err.contains("FEATURE_NOPE"), "{err}");
    }

    #[test]
    fn comments_whitespace_optional_colon_and_quotes() {
        let text = r#"
  # a comment with { } : and words
  sommelier : {   // optional colon, // comment
    condition { always : true } ;
    enable : x11_move_windows ,
    disable : "FEATURE_BLACK_SCREEN_FIX"
    enable: FEATURE_RANDR_EMU_FIX
  }
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(q.is_enabled(1, Feature::X11MoveWindows));
        assert!(!q.is_enabled(1, Feature::BlackScreenFix));
        assert!(q.is_enabled(1, Feature::RandrEmuFix));
    }

    #[test]
    fn load_comma_separated_files_merges_in_order() {
        let dir = std::env::temp_dir();
        let tag = format!("sommelier_quirks_{}", std::process::id());
        let p1 = dir.join(format!("{}_a.txt", tag));
        let p2 = dir.join(format!("{}_b.txt", tag));
        std::fs::write(
            &p1,
            "sommelier { condition { always: true } enable: FEATURE_BLACK_SCREEN_FIX }\n",
        )
        .unwrap();
        std::fs::write(
            &p2,
            "sommelier { condition { steam_game_id: 42 } enable: FEATURE_RANDR_EMU_FIX }\n",
        )
        .unwrap();

        let paths = format!("{},{}", p1.display(), p2.display());
        let q = Quirks::load_comma_separated_files(&paths).unwrap();
        assert!(q.is_enabled(1, Feature::BlackScreenFix));
        assert!(q.is_enabled(42, Feature::RandrEmuFix));
        assert!(!q.is_enabled(43, Feature::RandrEmuFix));

        // 打不开的文件被跳过，不影响其它文件。
        let missing = dir.join(format!("{}_missing.txt", tag));
        let paths = format!("{},{},{}", missing.display(), p1.display(), p2.display());
        let q = Quirks::load_comma_separated_files(&paths).unwrap();
        assert!(q.is_enabled(42, Feature::RandrEmuFix));

        let _ = std::fs::remove_file(&p1);
        let _ = std::fs::remove_file(&p2);
    }

    #[test]
    fn enabled_features_and_print_features() {
        let text = r#"
sommelier { condition { always: true } enable: FEATURE_RANDR_EMU_FIX }
sommelier { condition { always: true } enable: FEATURE_X11_MOVE_WINDOWS }
"#;
        let q = Quirks::parse(text).unwrap();
        assert_eq!(
            q.enabled_features(1),
            vec![Feature::X11MoveWindows, Feature::RandrEmuFix]
        );
        assert_eq!(
            q.print_features(1),
            "FEATURE_X11_MOVE_WINDOWS\nFEATURE_RANDR_EMU_FIX"
        );
        assert_eq!(Quirks::new().print_features(1), "");
    }

    #[test]
    fn rule_with_multiple_conditions_is_skipped() {
        let text = r#"
sommelier {
  condition { steam_game_id: 1 }
  condition { always: true }
  enable: FEATURE_X11_MOVE_WINDOWS
}
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(!q.is_enabled(1, Feature::X11MoveWindows));
        assert!(!q.is_enabled(2, Feature::X11MoveWindows));
    }

    #[test]
    fn invalid_number_error_includes_line_number() {
        let text = "sommelier {\n  condition { steam_game_id: not_a_number }\n  enable: FEATURE_X11_MOVE_WINDOWS\n}";
        let err = Quirks::parse(text).unwrap_err();
        assert!(err.contains("line 2"), "{err}");
    }

    #[test]
    fn empty_and_always_false_rules() {
        let q = Quirks::parse("").unwrap();
        assert_eq!(q.enabled_features(0), vec![]);
        assert!(!q.is_enabled(0, Feature::X11MoveWindows));

        // 主线：`always: false` 的条件不匹配，规则失效。
        let q = Quirks::parse(
            "sommelier { condition { always: false } enable: FEATURE_X11_MOVE_WINDOWS }",
        )
        .unwrap();
        assert!(!q.is_enabled(0, Feature::X11MoveWindows));
    }

    #[test]
    fn unknown_fields_and_blocks_are_tolerated() {
        let text = r#"
some_unknown_field: 42
sommelier {
  condition { steam_game_id: 9 unknown_nested { a: 1 } }
  future_field { nested { value: 1 } }
  enable: FEATURE_CONTAINERIZE_WINDOWS
}
"#;
        let q = Quirks::parse(text).unwrap();
        assert!(q.is_enabled(9, Feature::ContainerizeWindows));
        assert!(!q.is_enabled(10, Feature::ContainerizeWindows));
    }

    #[test]
    fn unterminated_block_error_includes_line_number() {
        let err = Quirks::parse("sommelier {\n  condition { always: true }\n").unwrap_err();
        assert!(err.contains("line"), "{err}");
    }
}
