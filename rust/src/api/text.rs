/// Small, bounded text operations are synchronous; disk/network operations are not.
#[flutter_rust_bridge::frb(sync)]
pub fn simplified_to_traditional(text: String) -> String {
    crate::text::convert(&text, false)
}

#[flutter_rust_bridge::frb(sync)]
pub fn traditional_to_simplified(text: String) -> String {
    crate::text::convert(&text, true)
}

#[flutter_rust_bridge::frb(sync)]
pub fn has_chinese_simplified(text: String) -> bool {
    crate::text::contains(&text, false)
}

#[flutter_rust_bridge::frb(sync)]
pub fn has_chinese_traditional(text: String) -> bool {
    crate::text::contains(&text, true)
}
