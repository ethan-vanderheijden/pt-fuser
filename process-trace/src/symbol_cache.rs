use std::collections::BTreeMap;

use pt_fuser::trace::SymbolInfo;

#[derive(Debug)]
pub struct SymbolCache {
    // ordered by symbol's vaddr_start
    cache: BTreeMap<u64, SymbolInfo>,
}

impl SymbolCache {
    pub fn new() -> Self {
        Self {
            cache: BTreeMap::new(),
        }
    }

    pub fn get(&self, ip: u64) -> Option<&SymbolInfo> {
        let (_, symbol_info) = self.cache.range(..=ip).next_back()?;
        if symbol_info.contains(ip) {
            Some(symbol_info)
        } else {
            None
        }
    }

    pub fn insert(&mut self, symbol_info: SymbolInfo) {
        self.cache.insert(symbol_info.vaddr_start, symbol_info);
    }
}
