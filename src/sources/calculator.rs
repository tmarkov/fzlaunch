use crate::config::CalculatorSourceConfig;
use crate::model::{Candidate, CandidateSource, Value};

pub struct Calculator {
    config: CalculatorSourceConfig,
}

impl Calculator {
    pub fn new(config: CalculatorSourceConfig) -> Self {
        Self { config }
    }

    pub fn candidates(&self, input: &str) -> Vec<Candidate> {
        let Some(expression) = calculator_expression(input) else {
            return Vec::new();
        };
        let Some(result) = calculate(&expression) else {
            return Vec::new();
        };

        vec![Candidate::new_with_action(
            Value::raw(result.clone()),
            '=',
            Some(self.config.direct_action.clone()),
        )
        .with_source(CandidateSource::Calculator)
        .with_haystack(format!(";= {expression} = {result}"))]
    }
}

fn calculator_expression(input: &str) -> Option<String> {
    let terms = input.split_whitespace().collect::<Vec<_>>();
    let trigger_index = terms.iter().rposition(|term| *term == ";=")?;

    Some(
        terms[..trigger_index]
            .iter()
            .copied()
            .filter(|term| *term != ";=")
            .collect::<Vec<_>>()
            .join(" "),
    )
    .filter(|expression| !expression.is_empty())
}

fn calculate(expression: &str) -> Option<String> {
    let mut parser = CalculatorParser::new(expression);
    let result = parser.parse_expression()?;
    parser.at_end().then(|| format_calculator_result(result))
}

fn format_calculator_result(result: f64) -> String {
    if (result.fract()).abs() < f64::EPSILON {
        return format!("{result:.0}");
    }

    let mut text = format!("{result:.12}");
    while text.contains('.') && text.ends_with('0') {
        text.pop();
    }
    if text.ends_with('.') {
        text.pop();
    }
    text
}

struct CalculatorParser<'a> {
    input: &'a str,
    position: usize,
}

impl<'a> CalculatorParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, position: 0 }
    }

    fn parse_expression(&mut self) -> Option<f64> {
        let mut value = self.parse_term()?;

        loop {
            self.skip_whitespace();
            if self.consume('+') {
                value += self.parse_term()?;
            } else if self.consume('-') {
                value -= self.parse_term()?;
            } else {
                return Some(value);
            }
        }
    }

    fn parse_term(&mut self) -> Option<f64> {
        let mut value = self.parse_factor()?;

        loop {
            self.skip_whitespace();
            if self.consume('*') {
                value *= self.parse_factor()?;
            } else if self.consume('/') {
                let divisor = self.parse_factor()?;
                if divisor == 0.0 {
                    return None;
                }
                value /= divisor;
            } else {
                return Some(value);
            }
        }
    }

    fn parse_factor(&mut self) -> Option<f64> {
        self.skip_whitespace();

        if self.consume('+') {
            return self.parse_factor();
        }
        if self.consume('-') {
            return Some(-self.parse_factor()?);
        }
        if self.consume('(') {
            let value = self.parse_expression()?;
            self.skip_whitespace();
            return self.consume(')').then_some(value);
        }

        self.parse_number()
    }

    fn parse_number(&mut self) -> Option<f64> {
        self.skip_whitespace();
        let start = self.position;
        let mut seen_digit = false;
        let mut seen_dot = false;

        while let Some(character) = self.peek() {
            if character.is_ascii_digit() {
                seen_digit = true;
                self.position += character.len_utf8();
            } else if character == '.' && !seen_dot {
                seen_dot = true;
                self.position += character.len_utf8();
            } else {
                break;
            }
        }

        if !seen_digit {
            return None;
        }

        self.input[start..self.position].parse().ok()
    }

    fn at_end(&mut self) -> bool {
        self.skip_whitespace();
        self.position == self.input.len()
    }

    fn consume(&mut self, expected: char) -> bool {
        if self.peek() == Some(expected) {
            self.position += expected.len_utf8();
            true
        } else {
            false
        }
    }

    fn peek(&self) -> Option<char> {
        self.input[self.position..].chars().next()
    }

    fn skip_whitespace(&mut self) {
        while let Some(character) = self.peek() {
            if !character.is_whitespace() {
                break;
            }
            self.position += character.len_utf8();
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::config::CalculatorSourceConfig;
    use crate::model::{Candidate, CandidateSource, Value};

    use super::Calculator;

    fn calculator() -> Calculator {
        Calculator::new(CalculatorSourceConfig::default())
    }

    #[test]
    fn calculator_source_requires_standalone_trigger() {
        assert_eq!(calculator().candidates("3+4;=+5"), Vec::<Candidate>::new());
        assert_eq!(calculator().candidates("3 + 4"), Vec::<Candidate>::new());
    }

    #[test]
    fn calculator_source_evaluates_expression_before_trigger() {
        assert_eq!(
            calculator().candidates("3 + 4 ;= + 5"),
            vec![Candidate::new(
                Value::raw("7"),
                '=',
                Some(Value::raw("printf %s {} | wl-copy"))
            )
            .with_source(CandidateSource::Calculator)
            .with_haystack(";= 3 + 4 = 7")]
        );
    }

    #[test]
    fn calculator_source_reentered_trigger_uses_terms_since_previous_trigger() {
        assert_eq!(
            calculator().candidates("3 + 4 ;= + 5 ;="),
            vec![Candidate::new(
                Value::raw("12"),
                '=',
                Some(Value::raw("printf %s {} | wl-copy"))
            )
            .with_source(CandidateSource::Calculator)
            .with_haystack(";= 3 + 4 + 5 = 12")]
        );
    }

    #[test]
    fn calculator_source_supports_operator_precedence_and_parentheses() {
        assert_eq!(
            calculator().candidates("(2 + 3) * 4 ;="),
            vec![Candidate::new(
                Value::raw("20"),
                '=',
                Some(Value::raw("printf %s {} | wl-copy"))
            )
            .with_source(CandidateSource::Calculator)
            .with_haystack(";= (2 + 3) * 4 = 20")]
        );
    }
}
