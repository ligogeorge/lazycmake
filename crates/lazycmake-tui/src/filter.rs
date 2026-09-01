pub struct FilterIndex {
    items: Vec<String>,
    query: String,
    visible: Vec<usize>,
}

impl FilterIndex {
    pub fn new(items: Vec<String>) -> Self {
        let visible: Vec<usize> = (0..items.len()).collect();
        Self {
            items,
            query: String::new(),
            visible,
        }
    }

    pub fn set_query(&mut self, query: &str) {
        self.query = query.to_string();
        if query.is_empty() {
            self.visible = (0..self.items.len()).collect();
            return;
        }

        let needle = query.to_lowercase();
        self.visible = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.to_lowercase().contains(&needle))
            .map(|(idx, _)| idx)
            .collect();
    }

    pub fn visible_indices(&self) -> &[usize] {
        &self.visible
    }

    pub fn query(&self) -> &str {
        &self.query
    }

    pub fn selected_item(&self, selected: usize) -> Option<&String> {
        self.visible
            .get(selected)
            .and_then(|&idx| self.items.get(idx))
    }

    pub fn items(&self) -> &[String] {
        &self.items
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filters_items() {
        let mut index = FilterIndex::new(vec!["alpha".into(), "beta".into(), "alphabet".into()]);
        index.set_query("alp");
        assert_eq!(index.visible_indices().len(), 2);
        assert_eq!(index.query(), "alp");
        assert_eq!(index.selected_item(0).map(String::as_str), Some("alpha"));
        assert_eq!(index.selected_item(1).map(String::as_str), Some("alphabet"));
    }

    #[test]
    fn filter_is_case_insensitive() {
        let mut index = FilterIndex::new(vec!["BatteryIndicatorTest".into(), "other".into()]);
        index.set_query("battery");
        assert_eq!(index.visible_indices(), &[0]);
    }

    #[test]
    fn empty_query_restores_all_items() {
        let mut index = FilterIndex::new(vec!["a".into(), "b".into(), "c".into()]);
        index.set_query("z");
        assert!(index.visible_indices().is_empty());
        index.set_query("");
        assert_eq!(index.visible_indices(), &[0, 1, 2]);
        assert!(index.selected_item(99).is_none());
    }
}
