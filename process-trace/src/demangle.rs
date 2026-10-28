use std::collections::HashMap;

fn demangle_name(name: &str) -> String {
    let is_plt = name.ends_with("@plt");
    let source = name.strip_suffix("@plt").unwrap_or(name);

    let mut result =
        if let Ok(Ok(demangled)) = cpp_demangle::Symbol::new(source).map(|s| s.demangle()) {
            demangled
        } else if let Ok(demangled) = rustc_demangle::try_demangle(source) {
            demangled.to_string()
        } else {
            name.to_string()
        };
    if is_plt {
        result.push_str("@plt");
    }
    result
}

#[derive(Debug)]
pub struct DemangleCache {
    cache: HashMap<String, String>,
}

impl DemangleCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    pub fn demangle(&mut self, name: &str) -> String {
        if let Some(cached) = self.cache.get(name) {
            return cached.clone();
        }
        let demangled = demangle_name(name);
        self.cache.insert(name.to_string(), demangled.clone());
        demangled
    }
}
