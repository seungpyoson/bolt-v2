use crate::bolt_v3_numeric::Probability;

pub(crate) fn number(value: f64) -> String {
    value.to_string()
}

pub(crate) fn probability(value: Probability) -> String {
    number(value.value())
}

pub(crate) fn optional_probability(value: Option<Probability>) -> Option<String> {
    value.map(probability)
}

pub(crate) fn optional_number(value: Option<f64>) -> Option<String> {
    value.filter(|value| value.is_finite()).map(number)
}
