with open("src/runner/backends/claude.rs", "r") as f:
    content = f.read()

content = content.replace(".and_then(|text| output::extract_claude_transcript_summary(text));", ".and_then(output::extract_claude_transcript_summary);")
content = content.replace("transcript_text.as_deref().map_or(false, |t| {", "transcript_text.as_deref().is_some_and(|t| {")

with open("src/runner/backends/claude.rs", "w") as f:
    f.write(content)
