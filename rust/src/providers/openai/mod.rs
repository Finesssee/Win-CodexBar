//! OpenAI/ChatGPT provider implementation
//!
//! Provides usage data scraping from OpenAI dashboard using browser automation.

pub mod friendly_errors;
pub mod scraper;

// Re-exports for error handling and dashboard scraping
#[allow(
    unused_imports,
    reason = "imports needed for future OpenAI provider wiring"
)]
pub use friendly_errors::{
    OpenAIWebErrorKind, extract_auth_status, extract_signed_in_email, friendly_error, is_logged_out,
};
#[allow(
    unused_imports,
    reason = "imports needed for future OpenAI provider wiring"
)]
pub use scraper::{
    CreditsHistoryEntry, OPENAI_DASHBOARD_SCRAPE_SCRIPT, OpenAIDashboardData, UsageBreakdown,
    parse_dashboard_json,
};
