use crate::{config::VerdictConfig, result::Line};
use std::collections::BTreeMap;
pub struct Stats;
pub enum Verdict {}
pub fn aggregate(_: &[Line]) -> BTreeMap<String, Stats> {
    BTreeMap::new()
}
pub fn verdict(_: &BTreeMap<String, Stats>, _: &VerdictConfig) -> Verdict {
    unimplemented!("Task 6")
}
pub fn render(_: &BTreeMap<String, Stats>, _: Option<&Verdict>) -> String {
    String::new()
}
