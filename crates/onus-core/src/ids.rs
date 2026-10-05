//! Stable ids for map nodes.
//!
//! Ids never contain absolute paths, so the same tree always produces the
//! same ids wherever it is checked out.

/// `<component>:<path>#<qualified name>`.
pub fn symbol_id(component: &str, path: &str, qualified: &str) -> String {
    format!("{component}:{path}#{qualified}")
}

/// `<component>:<path>`: a file, used for top-level code and imports.
pub fn module_id(component: &str, path: &str) -> String {
    format!("{component}:{path}")
}

pub fn event_id(name: &str) -> String {
    format!("event:{name}")
}

pub fn table_id(model: &str) -> String {
    format!("db-table:{model}")
}

pub fn config_key_id(key: &str) -> String {
    format!("config-key:{key}")
}

pub fn external_id(slug: &str) -> String {
    format!("external:{slug}")
}

pub fn npm_id(package: &str) -> String {
    format!("npm:{package}")
}

/// Prefixes of ids that do not belong to a component.
pub const GLOBAL_PREFIXES: &[&str] = &["event:", "db-table:", "config-key:", "external:", "npm:"];

pub fn is_global(id: &str) -> bool {
    GLOBAL_PREFIXES.iter().any(|p| id.starts_with(p))
}

/// The component part of a symbol or module id.
pub fn component_of(id: &str) -> Option<&str> {
    if is_global(id) {
        return None;
    }
    id.split_once(':').map(|(c, _)| c)
}

/// The path part (within the component) of a symbol or module id.
pub fn path_of(id: &str) -> Option<&str> {
    if is_global(id) {
        return None;
    }
    let rest = id.split_once(':')?.1;
    Some(rest.split_once('#').map_or(rest, |(p, _)| p))
}

/// The qualified name of a symbol id (`Class.method`, `UserPreferences`).
pub fn name_of(id: &str) -> &str {
    if let Some((_, name)) = id.split_once('#') {
        return name;
    }
    for p in GLOBAL_PREFIXES {
        if let Some(rest) = id.strip_prefix(p) {
            return rest;
        }
    }
    id
}

/// The module id a symbol id lives in.
pub fn module_of(id: &str) -> &str {
    id.split_once('#').map_or(id, |(m, _)| m)
}

/// A short, stable slug: lowercase ASCII letters, digits and dashes.
pub fn slug(text: &str) -> String {
    let mut out = String::new();
    let mut dash = false;
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_symbol_ids() {
        let id = symbol_id("user-preferences", "src/types.ts", "UserPreferences");
        assert_eq!(id, "user-preferences:src/types.ts#UserPreferences");
        assert_eq!(component_of(&id), Some("user-preferences"));
        assert_eq!(path_of(&id), Some("src/types.ts"));
        assert_eq!(name_of(&id), "UserPreferences");
        assert_eq!(module_of(&id), "user-preferences:src/types.ts");
    }

    #[test]
    fn global_ids_have_no_component() {
        assert_eq!(component_of(&event_id("OrderShipped")), None);
        assert_eq!(name_of(&event_id("OrderShipped")), "OrderShipped");
        assert_eq!(component_of("npm:@acme/sms"), None);
        assert_eq!(name_of("npm:@acme/sms"), "@acme/sms");
    }

    #[test]
    fn slugs_are_stable() {
        assert_eq!(slug("Acme SMS"), "acme-sms");
        assert_eq!(slug("@sendgrid/mail"), "sendgrid-mail");
        assert_eq!(slug("  --x--  "), "x");
    }
}
