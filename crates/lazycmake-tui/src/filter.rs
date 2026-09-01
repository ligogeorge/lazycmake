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

        let mut ranked: Vec<(MatchRank, usize)> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| match_rank(item, query).map(|rank| (rank, idx)))
            .collect();
        ranked.sort_by(|(rank_a, idx_a), (rank_b, idx_b)| {
            rank_a
                .cmp(rank_b)
                .then_with(|| {
                    self.items[*idx_a]
                        .to_lowercase()
                        .cmp(&self.items[*idx_b].to_lowercase())
                })
        });
        self.visible = ranked.into_iter().map(|(_, idx)| idx).collect();
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct MatchRank {
    /// 0 = literal substring, 1 = alnum-only fallback (`82` → `8-2`).
    kind: u8,
    /// Index of the match in the compared string (earlier is better).
    position: usize,
    /// Prefer shorter names when rank otherwise ties.
    name_len: usize,
}

fn match_rank(item: &str, query: &str) -> Option<MatchRank> {
    let item_l = item.to_lowercase();
    let query_l = query.to_lowercase();
    if query_l.is_empty() {
        return None;
    }

    if let Some(position) = item_l.find(&query_l) {
        return Some(MatchRank {
            kind: 0,
            position,
            name_len: item_l.len(),
        });
    }

    // Alnum fallback only when the query has no punctuation — otherwise `7-`
    // would collapse to `7` and match `TRV4-7…`.
    if query_l.chars().any(|c| !c.is_ascii_alphanumeric()) {
        return None;
    }
    let item_alnum: String = item_l.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let position = item_alnum.find(&query_l)?;
    Some(MatchRank {
        kind: 1,
        position,
        name_len: item_l.len(),
    })
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

    #[test]
    fn filter_matches_trv_preset_with_hyphen_queries() {
        let mut index = FilterIndex::new(vec![
            "TRV7-1Full".into(),
            "TRV8-2Full".into(),
            "tests".into(),
        ]);
        index.set_query("8-2");
        assert_eq!(
            index.selected_item(0).map(String::as_str),
            Some("TRV8-2Full")
        );
        index.set_query("82");
        assert_eq!(
            index.selected_item(0).map(String::as_str),
            Some("TRV8-2Full")
        );
        index.set_query("trv8");
        assert_eq!(
            index.selected_item(0).map(String::as_str),
            Some("TRV8-2Full")
        );
    }

    #[test]
    fn hyphen_query_does_not_match_unrelated_generation_suffixes() {
        let mut index = FilterIndex::new(vec![
            "TRV4-7Full".into(),
            "TRV6-7Full".into(),
            "TRV7-1Full".into(),
            "TRV7-2Full".into(),
        ]);
        index.set_query("7-");
        let names: Vec<&str> = (0..index.visible_indices().len())
            .filter_map(|i| index.selected_item(i).map(String::as_str))
            .collect();
        assert_eq!(names, vec!["TRV7-1Full", "TRV7-2Full"]);
    }

    #[test]
    fn ranks_generation_prefix_matches_before_suffix_digit() {
        let mut index = FilterIndex::new(vec![
            "TRV4-7Full".into(),
            "TRV6-7Full".into(),
            "TRV7-2Full".into(),
            "TRV7-1Full".into(),
        ]);
        index.set_query("7");
        let names: Vec<&str> = (0..index.visible_indices().len())
            .filter_map(|i| index.selected_item(i).map(String::as_str))
            .collect();
        assert_eq!(
            names,
            vec!["TRV7-1Full", "TRV7-2Full", "TRV4-7Full", "TRV6-7Full"]
        );
    }
}
