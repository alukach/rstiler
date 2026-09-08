//! Query parameters, read the way titiler reads them.
//!
//! titiler spells a repeated parameter by repeating it — `bidx=1&bidx=2` — so
//! a `HashMap<String, String>` silently keeps only the last one. Keep the pairs
//! and let a caller ask for all of them.

/// The query string as it arrived, order preserved, duplicates kept.
pub(crate) struct Query(Vec<(String, String)>);

impl Query {
    pub(crate) fn new(pairs: impl Iterator<Item = (String, String)>) -> Self {
        Self(pairs.collect())
    }

    /// Every value given for `key`, in order.
    ///
    /// A comma-separated value is split, so `bidx=1,2,3` and `bidx=1&bidx=2&bidx=3`
    /// mean the same thing. titiler only accepts the second; accepting both
    /// costs nothing and the first is what a hand-written URL tends to use.
    pub(crate) fn all(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, _)| k == key)
            .flat_map(|(_, v)| v.split(','))
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .collect()
    }

    /// The last value given for `key`, ignoring empty ones.
    ///
    /// Values are not split here: `rescale=0,1000` is one value, and a repeated
    /// `colormap=` should not be torn apart on the commas inside its JSON.
    pub(crate) fn last(&self, key: &str) -> Option<&str> {
        self.0
            .iter()
            .filter(|(k, v)| k == key && !v.trim().is_empty())
            .map(|(_, v)| v.trim())
            .next_back()
    }

    /// Every value for `key` kept whole, for parameters that are themselves
    /// comma-bearing — `rescale=0,1000&rescale=0,255`.
    pub(crate) fn whole(&self, key: &str) -> Vec<&str> {
        self.0
            .iter()
            .filter(|(k, v)| k == key && !v.trim().is_empty())
            .map(|(_, v)| v.trim())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Query {
        Query::new(s.split('&').filter(|p| !p.is_empty()).map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (k.to_string(), v.to_string())
        }))
    }

    #[test]
    fn repeated_and_comma_forms_agree() {
        assert_eq!(q("bidx=1&bidx=2&bidx=3").all("bidx"), ["1", "2", "3"]);
        assert_eq!(q("bidx=1,2,3").all("bidx"), ["1", "2", "3"]);
    }

    #[test]
    fn repeating_the_same_band_is_kept() {
        // titiler's own test asks for b1 three times to make a grey RGB.
        assert_eq!(q("bidx=1&bidx=1&bidx=1").all("bidx"), ["1", "1", "1"]);
    }

    #[test]
    fn last_wins_and_empties_are_ignored() {
        assert_eq!(q("a=1&a=2").last("a"), Some("2"));
        assert_eq!(q("a=&a=7").last("a"), Some("7"));
        assert_eq!(q("a=").last("a"), None);
        assert_eq!(q("").last("a"), None);
    }

    #[test]
    fn whole_keeps_commas_inside_a_value() {
        assert_eq!(q("rescale=0,1000").whole("rescale"), ["0,1000"]);
        assert_eq!(
            q("rescale=0,1000&rescale=5,6").whole("rescale"),
            ["0,1000", "5,6"]
        );
    }
}
