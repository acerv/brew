// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 Andrea Cervesato <andrea.cervesato@suse.com>

/// An email address with optional display name.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Address {
    /// Display name (e.g. "Alice"). Empty when not provided.
    name: String,
    /// Email address (e.g. "alice@example.com"). Empty when not provided.
    addr: String,
}

impl Address {
    pub fn new(name: &str, addr: &str) -> Self {
        Self {
            name: name.to_string(),
            addr: addr.to_string(),
        }
    }

    #[cfg(test)]
    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn address(&self) -> &str {
        &self.addr
    }

    /// Return the full header form: `"Name <addr>"` when a name is present,
    /// otherwise just the bare address.
    pub fn full(&self) -> String {
        if self.name.is_empty() {
            self.addr.clone()
        } else {
            format!("{} <{}>", self.name, self.addr)
        }
    }

    /// Return a short display form: the name if present, otherwise the address.
    pub fn short(&self) -> &str {
        if self.name.is_empty() {
            &self.addr
        } else {
            &self.name
        }
    }

    /// Split a comma-separated address list into individual `Address` values,
    /// respecting angle brackets so that `"Doe, John <j@x.com>, alice@x.com"`
    /// splits into two entries rather than three.
    pub fn parse_list(list: &str) -> Vec<Self> {
        let mut result = Vec::new();
        let mut current = String::new();
        let mut depth = 0u32;

        for ch in list.chars() {
            match ch {
                '<' => {
                    depth += 1;
                    current.push(ch);
                }
                '>' => {
                    depth = depth.saturating_sub(1);
                    current.push(ch);
                }
                ',' if depth == 0 => {
                    let t = current.trim().to_string();
                    if !t.is_empty() {
                        result.push(t.parse::<Address>().unwrap());
                    }
                    current.clear();
                }
                _ => current.push(ch),
            }
        }
        let t = current.trim().to_string();
        if !t.is_empty() {
            result.push(t.parse::<Address>().unwrap());
        }
        result
    }
}

impl<'a> From<&'a mail_parser::Addr<'a>> for Address {
    fn from(addr: &'a mail_parser::Addr<'a>) -> Self {
        Address::new(
            addr.name().unwrap_or_default(),
            addr.address().unwrap_or_default(),
        )
    }
}

impl std::fmt::Display for Address {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.full())
    }
}

impl std::str::FromStr for Address {
    type Err = std::convert::Infallible;

    /// Parse an address from a string like `"Name <addr>"` or `"addr"`.
    /// Never fails — returns an empty `Address` for blank input.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.is_empty() {
            return Ok(Self::default());
        }

        if let Some(start) = s.find('<')
            && let Some(end) = s[start..].find('>')
        {
            let addr = s[start + 1..start + end].trim();
            let name = s[..start].trim();
            return Ok(Self::new(name, addr));
        }

        // Bare address (no angle brackets)
        Ok(Self::new("", s))
    }
}

/// A persistent address book stored as a plain text file.
pub struct AddressBook {
    entries: Vec<Address>,
    path: std::path::PathBuf,
}

impl Default for AddressBook {
    fn default() -> Self {
        Self {
            entries: Vec::new(),
            path: std::path::PathBuf::new(),
        }
    }
}

impl AddressBook {
    /// Load the address book from `~/.config/brew/addresses`.
    /// Returns an empty book if the file doesn't exist.
    pub fn load() -> Self {
        let path = crate::core::config::config_dir().join("addresses");
        let entries = std::fs::read_to_string(&path)
            .unwrap_or_default()
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| l.parse::<Address>().ok())
            .filter(|a| !a.full().is_empty())
            .collect();
        Self { entries, path }
    }

    /// Add addresses that aren't already in the book and save.
    pub fn harvest(&mut self, addrs: &[Address]) {
        let mut changed = false;
        for addr in addrs {
            if addr.full().is_empty() {
                continue;
            }
            if !self.entries.iter().any(|e| e.full() == addr.full()) {
                self.entries.push(addr.clone());
                changed = true;
            }
        }
        if changed {
            self.save();
        }
    }

    /// Fuzzy search: match addresses where name or email contains the query
    /// (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<&Address> {
        if query.is_empty() {
            return Vec::new();
        }
        let q = query.to_lowercase();
        self.entries
            .iter()
            .filter(|a| a.full().to_lowercase().contains(&q))
            .collect()
    }

    fn save(&self) {
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let content: String = self
            .entries
            .iter()
            .map(|a| format!("{}\n", a.full()))
            .collect();
        let _ = std::fs::write(&self.path, content);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── new ──────────────────────────────────────────────────────────────────

    #[test]
    fn new_stores_name_and_addr() {
        let a = Address::new("Alice", "alice@example.com");
        assert_eq!(a.name(), "Alice");
        assert_eq!(a.address(), "alice@example.com");
    }

    // ── full ─────────────────────────────────────────────────────────────────

    #[test]
    fn full_with_name() {
        let a = Address::new("Alice", "alice@example.com");
        assert_eq!(a.full(), "Alice <alice@example.com>");
    }

    #[test]
    fn full_without_name() {
        let a = Address::new("", "alice@example.com");
        assert_eq!(a.full(), "alice@example.com");
    }

    // ── short ────────────────────────────────────────────────────────────────

    #[test]
    fn short_with_name() {
        let a = Address::new("Alice", "alice@example.com");
        assert_eq!(a.short(), "Alice");
    }

    #[test]
    fn short_without_name() {
        let a = Address::new("", "alice@example.com");
        assert_eq!(a.short(), "alice@example.com");
    }

    // ── Display ──────────────────────────────────────────────────────────────

    #[test]
    fn display_trait_uses_full() {
        let a = Address::new("Alice", "alice@example.com");
        assert_eq!(format!("{}", a), "Alice <alice@example.com>");
    }

    // ── Default ──────────────────────────────────────────────────────────────

    #[test]
    fn default_is_empty() {
        let a = Address::default();
        assert_eq!(a.name(), "");
        assert_eq!(a.address(), "");
        assert_eq!(a.full(), "");
    }

    // ── FromStr ──────────────────────────────────────────────────────────────

    #[test]
    fn from_str_name_and_addr() {
        let a: Address = "Alice <alice@example.com>".parse().unwrap();
        assert_eq!(a.name(), "Alice");
        assert_eq!(a.address(), "alice@example.com");
    }

    #[test]
    fn from_str_name_with_spaces() {
        let a: Address = "John Doe <john@example.com>".parse().unwrap();
        assert_eq!(a.name(), "John Doe");
        assert_eq!(a.address(), "john@example.com");
    }

    #[test]
    fn from_str_bare_address() {
        let a: Address = "alice@example.com".parse().unwrap();
        assert_eq!(a.name(), "");
        assert_eq!(a.address(), "alice@example.com");
    }

    #[test]
    fn from_str_with_whitespace() {
        let a: Address = "  Alice  <alice@example.com>  ".parse().unwrap();
        assert_eq!(a.name(), "Alice");
        assert_eq!(a.address(), "alice@example.com");
    }

    #[test]
    fn from_str_empty() {
        let a: Address = "".parse().unwrap();
        assert_eq!(a.name(), "");
        assert_eq!(a.address(), "");
    }

    #[test]
    fn from_str_roundtrip() {
        let original = Address::new("Alice", "alice@example.com");
        let parsed: Address = original.full().parse().unwrap();
        assert_eq!(parsed, original);
    }

    // ── split_addresses ──────────────────────────────────────────────────────

    #[test]
    fn split_single_bare() {
        let addrs = Address::parse_list("alice@example.com");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].address(), "alice@example.com");
    }

    #[test]
    fn split_two_bare() {
        let addrs = Address::parse_list("alice@x.com, bob@x.com");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].address(), "alice@x.com");
        assert_eq!(addrs[1].address(), "bob@x.com");
    }

    #[test]
    fn split_preserves_name_with_angle_brackets() {
        let addrs = Address::parse_list("John Doe <john@x.com>, alice@x.com");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].name(), "John Doe");
        assert_eq!(addrs[0].address(), "john@x.com");
        assert_eq!(addrs[1].address(), "alice@x.com");
    }

    #[test]
    fn split_mixed() {
        let addrs = Address::parse_list("Alice <a@x.com>, b@x.com, Carol <c@x.com>");
        assert_eq!(addrs.len(), 3);
        assert_eq!(addrs[0].name(), "Alice");
        assert_eq!(addrs[1].address(), "b@x.com");
        assert_eq!(addrs[2].name(), "Carol");
    }

    #[test]
    fn split_empty() {
        let addrs = Address::parse_list("");
        assert!(addrs.is_empty());
    }

    #[test]
    fn split_whitespace_only() {
        let addrs = Address::parse_list("   ");
        assert!(addrs.is_empty());
    }

    // ── AddressBook ─────────────────────────────────────────────────────────

    fn temp_book(content: &str) -> AddressBook {
        let dir = std::env::temp_dir().join(format!(
            "brew_ab_test_{}_{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("addresses");
        if !content.is_empty() {
            std::fs::write(&path, content).unwrap();
        }
        AddressBook {
            entries: content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| l.parse::<Address>().ok())
                .filter(|a| !a.full().is_empty())
                .collect(),
            path,
        }
    }

    #[test]
    fn search_matches_name() {
        let book = temp_book("Alice <alice@x.com>\nBob <bob@x.com>\n");
        let results = book.search("ali");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].short(), "Alice");
    }

    #[test]
    fn search_matches_email() {
        let book = temp_book("Alice <alice@x.com>\nBob <bob@y.com>\n");
        let results = book.search("y.com");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].short(), "Bob");
    }

    #[test]
    fn search_is_case_insensitive() {
        let book = temp_book("Alice <alice@x.com>\n");
        assert_eq!(book.search("ALICE").len(), 1);
    }

    #[test]
    fn search_empty_query_returns_nothing() {
        let book = temp_book("Alice <alice@x.com>\n");
        assert!(book.search("").is_empty());
    }

    #[test]
    fn harvest_adds_new_addresses() {
        let mut book = temp_book("");
        book.harvest(&[Address::new("Alice", "alice@x.com")]);
        assert_eq!(book.entries.len(), 1);
        let saved = std::fs::read_to_string(&book.path).unwrap();
        assert!(saved.contains("alice@x.com"));
    }

    #[test]
    fn harvest_skips_duplicates() {
        let mut book = temp_book("Alice <alice@x.com>\n");
        book.harvest(&[Address::new("Alice", "alice@x.com")]);
        assert_eq!(book.entries.len(), 1);
    }

    #[test]
    fn harvest_skips_empty() {
        let mut book = temp_book("");
        book.harvest(&[Address::default()]);
        assert!(book.entries.is_empty());
    }

    // ── From<&mail_parser::Addr> ─────────────────────────────────────────────

    fn parse_msg(raw: &str) -> mail_parser::Message<'static> {
        mail_parser::MessageParser::default()
            .parse(raw.as_bytes())
            .unwrap()
            .into_owned()
    }

    #[test]
    fn from_mail_parser_addr_with_name() {
        let raw = "From: Alice <alice@x.com>\r\nTo: Bob <bob@x.com>\r\n\r\n";
        let msg = parse_msg(raw);
        let addrs: Vec<Address> = msg
            .to()
            .map(|a| a.iter().map(Address::from).collect())
            .unwrap_or_default();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].address(), "bob@x.com");
        assert_eq!(addrs[0].full(), "Bob <bob@x.com>");
    }

    #[test]
    fn from_mail_parser_addr_bare_address() {
        let raw = "From: alice@x.com\r\nTo: bob@x.com\r\n\r\n";
        let msg = parse_msg(raw);
        let addrs: Vec<Address> = msg
            .to()
            .map(|a| a.iter().map(Address::from).collect())
            .unwrap_or_default();
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].address(), "bob@x.com");
        assert_eq!(addrs[0].full(), "bob@x.com");
    }

    #[test]
    fn from_mail_parser_addr_multiple_recipients() {
        let raw = "From: a@x.com\r\nTo: Alice <alice@x.com>, bob@x.com\r\n\r\n";
        let msg = parse_msg(raw);
        let addrs: Vec<Address> = msg
            .to()
            .map(|a| a.iter().map(Address::from).collect())
            .unwrap_or_default();
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].address(), "alice@x.com");
        assert_eq!(addrs[1].address(), "bob@x.com");
    }

    #[test]
    fn from_mail_parser_addr_none_yields_empty() {
        let raw = "From: a@x.com\r\n\r\n";
        let msg = parse_msg(raw);
        let addrs: Vec<Address> = msg
            .to()
            .map(|a| a.iter().map(Address::from).collect())
            .unwrap_or_default();
        assert!(addrs.is_empty());
    }
}
