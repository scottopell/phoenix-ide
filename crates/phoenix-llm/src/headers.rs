pub(crate) fn has_custom_source_header(custom_headers: &[(String, String)]) -> bool {
    custom_headers
        .iter()
        .any(|(key, _)| key.eq_ignore_ascii_case("source"))
}

pub(crate) fn apply_source_header(
    mut builder: reqwest::RequestBuilder,
    custom_headers: &[(String, String)],
) -> reqwest::RequestBuilder {
    if !has_custom_source_header(custom_headers) {
        builder = builder.header("source", phoenix_core::domain::llm_types::LLM_SOURCE_HEADER);
    }
    for (key, value) in custom_headers {
        builder = builder.header(key.as_str(), value.as_str());
    }
    builder
}
