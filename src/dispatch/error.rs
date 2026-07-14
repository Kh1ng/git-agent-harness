use super::text::utf8_safe_prefix;

pub(in crate::dispatch) fn summarize_error(err: &anyhow::Error) -> String {
    let text = format!("{:#}", err).replace('\n', " ");
    if text.len() > 500 {
        let safe_text = utf8_safe_prefix(&text, 497).to_string();
        format!("{safe_text}...")
    } else {
        text
    }
}
