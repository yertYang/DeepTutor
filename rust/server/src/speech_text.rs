//! Prepare mathematical expressions for speech without altering ordinary prose.
struct Math<'a> {
    input: &'a str,
    chinese: bool,
}
impl<'a> Math<'a> {
    fn word(&self, en: &str, zh: &str) -> String {
        if self.chinese { zh } else { en }.to_owned()
    }
    fn skip_space(&mut self) {
        self.input = self.input.trim_start();
    }
    fn argument(&mut self, depth: usize) -> String {
        if depth > 32 {
            return self.expression(depth, false);
        }
        self.skip_space();
        if self.input.starts_with('{') {
            self.input = &self.input[1..];
            self.expression(depth + 1, true)
        } else if self.input.starts_with('\\') {
            self.command(depth + 1)
        } else {
            let value = self.input.chars().next().unwrap_or(' ');
            self.input = &self.input[value.len_utf8().min(self.input.len())..];
            value.to_string()
        }
    }
    fn command(&mut self, depth: usize) -> String {
        self.input = &self.input[1..];
        let end = self
            .input
            .find(|c: char| !c.is_ascii_alphabetic())
            .unwrap_or(self.input.len());
        let name = &self.input[..end];
        self.input = &self.input[end..];
        match name {
            "frac" | "dfrac" | "tfrac" => {
                let top = self.argument(depth);
                let bottom = self.argument(depth);
                if self.chinese {
                    format!("{bottom} 分之 {top}")
                } else {
                    format!("fraction {top} over {bottom} end fraction")
                }
            }
            "sqrt" => {
                self.skip_space();
                let index = self.input.strip_prefix('[').and_then(|rest| {
                    rest.find(']')
                        .map(|end| (rest[..end].to_owned(), &rest[end + 1..]))
                });
                let root = if let Some((index, rest)) = index {
                    self.input = rest;
                    index
                } else {
                    String::new()
                };
                let value = self.argument(depth);
                if root.is_empty() {
                    format!("{} {value}", self.word("square root of", "根号"))
                } else {
                    format!("{root} {} {value}", self.word("root of", "次根号"))
                }
            }
            "text" | "mathrm" | "mathbf" | "mathit" | "operatorname" | "boxed" => {
                self.argument(depth)
            }
            "left" | "right" | "displaystyle" | "limits" | "quad" | "qquad" => String::new(),
            "alpha" => self.word("alpha", "阿尔法"),
            "beta" => self.word("beta", "贝塔"),
            "gamma" | "Gamma" => self.word("gamma", "伽马"),
            "delta" | "Delta" => self.word("delta", "德尔塔"),
            "theta" | "Theta" => self.word("theta", "西塔"),
            "lambda" => self.word("lambda", "拉姆达"),
            "mu" => self.word("mu", "缪"),
            "sigma" | "Sigma" => self.word("sigma", "西格玛"),
            "pi" => self.word("pi", "派"),
            "omega" | "Omega" => self.word("omega", "欧米伽"),
            "epsilon" | "varepsilon" => self.word("epsilon", "艾普西隆"),
            "phi" | "varphi" => self.word("phi", "斐"),
            "times" | "cdot" => self.word("times", "乘以"),
            "div" => self.word("divided by", "除以"),
            "pm" => self.word("plus or minus", "加减"),
            "le" | "leq" => self.word("less than or equal to", "小于等于"),
            "ge" | "geq" => self.word("greater than or equal to", "大于等于"),
            "ne" | "neq" => self.word("not equal to", "不等于"),
            "approx" => self.word("approximately equals", "约等于"),
            "infty" => self.word("infinity", "无穷"),
            "sum" => self.word("sum", "求和"),
            "prod" => self.word("product", "连乘"),
            "int" => self.word("integral", "积分"),
            "sin" | "cos" | "tan" | "log" | "ln" | "lim" => name.to_owned(),
            "to" | "rightarrow" => self.word("approaches", "趋于"),
            "in" => self.word("in", "属于"),
            "partial" => self.word("partial", "偏导"),
            "nabla" => self.word("nabla", "纳布拉"),
            "" => self.argument(depth),
            _ => name.to_owned(),
        }
    }
    fn expression(&mut self, depth: usize, grouped: bool) -> String {
        // Malformed or deeply nested input must remain bounded.
        if depth > 32 {
            let rest = self.input.replace(['{', '}', '\\', '$'], " ");
            self.input = "";
            return rest;
        }
        let mut pieces = Vec::new();
        while let Some(value) = self.input.chars().next() {
            if value == '}' && grouped {
                self.input = &self.input[1..];
                break;
            }
            let item = match value {
                '\\' => self.command(depth),
                '{' => {
                    self.input = &self.input[1..];
                    self.expression(depth + 1, true)
                }
                '^' | '_' => {
                    self.input = &self.input[1..];
                    let exponent = self.argument(depth);
                    if value == '_' {
                        format!("{} {exponent}", self.word("subscript", "下标"))
                    } else {
                        match exponent.trim() {
                            "2" => self.word("squared", "的平方"),
                            "3" => self.word("cubed", "的立方"),
                            _ => format!(
                                "{} {exponent}",
                                self.word("to the power of", "的次方，指数为")
                            ),
                        }
                    }
                }
                _ => {
                    self.input = &self.input[value.len_utf8()..];
                    match value {
                        '+' => self.word("plus", "加"),
                        '-' | '−' => self.word("minus", "减"),
                        '=' => self.word("equals", "等于"),
                        '<' => self.word("less than", "小于"),
                        '>' => self.word("greater than", "大于"),
                        '/' => self.word("divided by", "除以"),
                        '*' | '×' => self.word("times", "乘以"),
                        '(' => self.word("open parenthesis", "左括号"),
                        ')' => self.word("close parenthesis", "右括号"),
                        '$' | '}' => String::new(),
                        _ => value.to_string(),
                    }
                }
            };
            // Keep contiguous letters/digits together while separating spoken operators.
            if value.is_alphanumeric() || value.is_whitespace() {
                pieces.push(item);
            } else {
                pieces.push(format!(" {item} "));
            }
        }
        pieces
            .concat()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    }
}

pub fn prepare(input: &str, enabled: bool, chinese: bool) -> String {
    let mut rest = input;
    let mut output = String::new();
    while !rest.is_empty() {
        let delimiter = ["$$", "$", "\\(", "\\["]
            .into_iter()
            .filter_map(|open| rest.find(open).map(|pos| (pos, open)))
            .min_by_key(|(pos, _)| *pos);
        let Some((pos, open)) = delimiter else {
            output.push_str(rest);
            break;
        };
        output.push_str(&rest[..pos]);
        rest = &rest[pos + open.len()..];
        let close = match open {
            "\\(" => "\\)",
            "\\[" => "\\]",
            _ => open,
        };
        if let Some(end) = rest.find(close) {
            let formula = &rest[..end];
            if enabled {
                output.push_str(
                    &Math {
                        input: formula,
                        chinese,
                    }
                    .expression(0, false),
                );
            } else {
                output.push_str(formula);
            }
            rest = &rest[end + close.len()..];
        } else {
            output.push_str(open.trim_matches('$'));
        }
    }
    output.replace('$', "")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speaks_nested_formulas_and_preserves_prose() {
        assert_eq!(
            prepare("Use $\\frac{1}{\\sqrt{x^2+1}}=\\alpha$ now.", true, false),
            "Use fraction 1 over square root of x squared plus 1 end fraction equals alpha now."
        );
        assert_eq!(
            prepare("计算 \\(\\frac{1}{2}+x^2\\)。", true, true),
            "计算 2 分之 1 加 x 的平方。"
        );
        assert_eq!(
            prepare("Keep $x^2$ and \\[y+1\\].", false, false),
            "Keep x^2 and y+1."
        );
        assert_eq!(prepare("Costs $5 today", true, false), "Costs 5 today");
        assert!(prepare(&format!("${}$", "\\".repeat(4096)), true, false).len() < 10000);
        let nested = format!("$ {} $", "\\frac{".repeat(100));
        assert!(prepare(&nested, true, false).len() < 10000);
    }
}
