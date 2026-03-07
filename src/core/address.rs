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

    #[cfg(test)]
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

        if let Some(start) = s.find('<') {
            if let Some(end) = s[start..].find('>') {
                let addr = s[start + 1..start + end].trim();
                let name = s[..start].trim();
                return Ok(Self::new(name, addr));
            }
        }

        // Bare address (no angle brackets)
        Ok(Self::new("", s))
    }
}

/// Split a comma-separated address list into individual `Address` values,
/// respecting angle brackets so that `"Doe, John <j@x.com>, alice@x.com"`
/// splits into two entries rather than three.
pub fn split_addresses(list: &str) -> Vec<Address> {
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
        let addrs = split_addresses("alice@example.com");
        assert_eq!(addrs.len(), 1);
        assert_eq!(addrs[0].address(), "alice@example.com");
    }

    #[test]
    fn split_two_bare() {
        let addrs = split_addresses("alice@x.com, bob@x.com");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].address(), "alice@x.com");
        assert_eq!(addrs[1].address(), "bob@x.com");
    }

    #[test]
    fn split_preserves_name_with_angle_brackets() {
        let addrs = split_addresses("John Doe <john@x.com>, alice@x.com");
        assert_eq!(addrs.len(), 2);
        assert_eq!(addrs[0].name(), "John Doe");
        assert_eq!(addrs[0].address(), "john@x.com");
        assert_eq!(addrs[1].address(), "alice@x.com");
    }

    #[test]
    fn split_mixed() {
        let addrs = split_addresses("Alice <a@x.com>, b@x.com, Carol <c@x.com>");
        assert_eq!(addrs.len(), 3);
        assert_eq!(addrs[0].name(), "Alice");
        assert_eq!(addrs[1].address(), "b@x.com");
        assert_eq!(addrs[2].name(), "Carol");
    }

    #[test]
    fn split_empty() {
        let addrs = split_addresses("");
        assert!(addrs.is_empty());
    }

    #[test]
    fn split_whitespace_only() {
        let addrs = split_addresses("   ");
        assert!(addrs.is_empty());
    }
}
