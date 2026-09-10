//! Band math, the `expression=` parameter.
//!
//! titiler evaluates these with numexpr, so its grammar is whatever numpy
//! accepts. This implements the arithmetic subset, which is what the common
//! cases need — `(b8-b4)/(b8+b4)` for NDVI, `b1*0.0001` for scaling — and
//! rejects anything else rather than guessing at it.
//!
//! Dependency-free so it can be checked without a wasm toolchain:
//! `rustc --test src/expression.rs -o /tmp/e && /tmp/e`

/// One parsed expression, evaluated per pixel against a slice of band values.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Expr {
    /// A band, stored 0-based although it is written 1-based.
    Band(usize),
    Number(f64),
    Neg(Box<Expr>),
    Add(Box<Expr>, Box<Expr>),
    Sub(Box<Expr>, Box<Expr>),
    Mul(Box<Expr>, Box<Expr>),
    Div(Box<Expr>, Box<Expr>),
}

impl Expr {
    /// `bands[i]` is the value of `b{i+1}`. Division by zero yields NaN, which
    /// the caller treats as nodata — the alternative is a tile that fails
    /// wholesale because one pixel had a zero denominator.
    pub(crate) fn eval(&self, bands: &[f64]) -> f64 {
        match self {
            Expr::Band(i) => bands.get(*i).copied().unwrap_or(f64::NAN),
            Expr::Number(n) => *n,
            Expr::Neg(e) => -e.eval(bands),
            Expr::Add(a, b) => a.eval(bands) + b.eval(bands),
            Expr::Sub(a, b) => a.eval(bands) - b.eval(bands),
            Expr::Mul(a, b) => a.eval(bands) * b.eval(bands),
            Expr::Div(a, b) => {
                let d = b.eval(bands);
                if d == 0.0 {
                    f64::NAN
                } else {
                    a.eval(bands) / d
                }
            }
        }
    }

    /// Highest 1-based band the expression reads, for validating against the
    /// dataset before any pixels are touched.
    pub(crate) fn max_band(&self) -> usize {
        match self {
            Expr::Band(i) => i + 1,
            Expr::Number(_) => 0,
            Expr::Neg(e) => e.max_band(),
            Expr::Add(a, b) | Expr::Sub(a, b) | Expr::Mul(a, b) | Expr::Div(a, b) => {
                a.max_band().max(b.max_band())
            }
        }
    }
}

/// Parse `expression=`, which is one or more expressions separated by `;`.
/// titiler renders one output band per expression.
pub(crate) fn parse_all(src: &str) -> Result<Vec<Expr>, String> {
    let parts: Vec<&str> = src
        .split(';')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    if parts.is_empty() {
        return Err("expression is empty".into());
    }
    parts.iter().map(|p| parse(p)).collect()
}

pub(crate) fn parse(src: &str) -> Result<Expr, String> {
    let tokens = lex(src)?;
    let mut p = Parser { tokens, at: 0 };
    let e = p.expr()?;
    if p.at != p.tokens.len() {
        return Err(format!("unexpected {:?} in {src:?}", p.tokens[p.at]));
    }
    Ok(e)
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Band(usize),
    Number(f64),
    Plus,
    Minus,
    Star,
    Slash,
    Open,
    Close,
}

fn lex(src: &str) -> Result<Vec<Tok>, String> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' => i += 1,
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' => {
                out.push(Tok::Star);
                i += 1;
            }
            '/' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '(' => {
                out.push(Tok::Open);
                i += 1;
            }
            ')' => {
                out.push(Tok::Close);
                i += 1;
            }
            'b' | 'B' => {
                let start = i;
                i += 1;
                let mut n = String::new();
                while i < chars.len() && chars[i].is_ascii_digit() {
                    n.push(chars[i]);
                    i += 1;
                }
                let idx: usize = n
                    .parse()
                    .map_err(|_| format!("expected a band number after {:?}", chars[start]))?;
                if idx == 0 {
                    return Err("bands are numbered from b1".into());
                }
                out.push(Tok::Band(idx - 1));
            }
            d if d.is_ascii_digit() || d == '.' => {
                let mut n = String::new();
                while i < chars.len()
                    && (chars[i].is_ascii_digit()
                        || chars[i] == '.'
                        || chars[i] == 'e'
                        || chars[i] == 'E'
                        // an exponent's sign, but not a subtraction
                        || ((chars[i] == '+' || chars[i] == '-')
                            && matches!(chars[i - 1], 'e' | 'E')))
                {
                    n.push(chars[i]);
                    i += 1;
                }
                out.push(Tok::Number(
                    n.parse().map_err(|_| format!("{n:?} is not a number"))?,
                ));
            }
            other => return Err(format!("unsupported character {other:?} in expression")),
        }
    }
    if out.is_empty() {
        return Err("expression is empty".into());
    }
    Ok(out)
}

struct Parser {
    tokens: Vec<Tok>,
    at: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.at)
    }

    fn expr(&mut self) -> Result<Expr, String> {
        let mut lhs = self.term()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Tok::Plus => {
                    self.at += 1;
                    lhs = Expr::Add(Box::new(lhs), Box::new(self.term()?));
                }
                Tok::Minus => {
                    self.at += 1;
                    lhs = Expr::Sub(Box::new(lhs), Box::new(self.term()?));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn term(&mut self) -> Result<Expr, String> {
        let mut lhs = self.factor()?;
        while let Some(op) = self.peek().cloned() {
            match op {
                Tok::Star => {
                    self.at += 1;
                    lhs = Expr::Mul(Box::new(lhs), Box::new(self.factor()?));
                }
                Tok::Slash => {
                    self.at += 1;
                    lhs = Expr::Div(Box::new(lhs), Box::new(self.factor()?));
                }
                _ => break,
            }
        }
        Ok(lhs)
    }

    fn factor(&mut self) -> Result<Expr, String> {
        match self.peek().cloned() {
            Some(Tok::Minus) => {
                self.at += 1;
                Ok(Expr::Neg(Box::new(self.factor()?)))
            }
            Some(Tok::Number(n)) => {
                self.at += 1;
                Ok(Expr::Number(n))
            }
            Some(Tok::Band(i)) => {
                self.at += 1;
                Ok(Expr::Band(i))
            }
            Some(Tok::Open) => {
                self.at += 1;
                let e = self.expr()?;
                if self.peek() != Some(&Tok::Close) {
                    return Err("unbalanced parenthesis".into());
                }
                self.at += 1;
                Ok(e)
            }
            other => Err(match other {
                Some(t) => format!("unexpected {t:?}"),
                None => "expression ends early".into(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(src: &str, bands: &[f64]) -> f64 {
        parse(src).unwrap().eval(bands)
    }

    #[test]
    fn bands_are_one_based() {
        assert_eq!(ev("b1", &[7.0, 9.0]), 7.0);
        assert_eq!(ev("b2", &[7.0, 9.0]), 9.0);
        assert!(parse("b0").is_err(), "b0 is not a band");
    }

    #[test]
    fn arithmetic_respects_precedence() {
        assert_eq!(ev("1+2*3", &[]), 7.0);
        assert_eq!(ev("(1+2)*3", &[]), 9.0);
        assert_eq!(ev("8/2/2", &[]), 2.0, "division is left-associative");
        assert_eq!(ev("10-3-2", &[]), 5.0, "subtraction is left-associative");
    }

    #[test]
    fn ndvi_is_the_case_that_matters() {
        // (nir - red) / (nir + red)
        let v = ev("(b2-b1)/(b2+b1)", &[0.2, 0.6]);
        assert!((v - 0.5).abs() < 1e-12, "got {v}");
    }

    #[test]
    fn unary_minus_and_exponents() {
        assert_eq!(ev("-b1", &[3.0]), -3.0);
        assert_eq!(ev("-(b1-5)", &[3.0]), 2.0);
        assert_eq!(ev("b1*1e-4", &[10000.0]), 1.0);
        assert_eq!(ev("2e2", &[]), 200.0);
    }

    #[test]
    fn divide_by_zero_is_nodata_not_an_error() {
        assert!(ev("b1/b2", &[1.0, 0.0]).is_nan());
    }

    #[test]
    fn max_band_is_what_gets_validated() {
        assert_eq!(parse("(b8-b4)/(b8+b4)").unwrap().max_band(), 8);
        assert_eq!(parse("42").unwrap().max_band(), 0);
    }

    #[test]
    fn semicolons_separate_output_bands() {
        let all = parse_all("b1;b1*2;b1*3").unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(all[1].eval(&[5.0]), 10.0);
    }

    #[test]
    fn nonsense_is_rejected_rather_than_guessed_at() {
        for bad in ["", "b1 +", "(b1", "b1)", "b1 $ b2", "where(b1>0,1,0)", "b"] {
            assert!(parse(bad).is_err(), "{bad:?} should not parse");
        }
    }
}
