const isLlmChanged = existingLlmBaseUrl && existingLlmBaseUrl !== baseUrl;
const existingEmbedBaseUrl = doc.getIn(['memory', 'embedding', 'baseUrl']);
const isEmbedChanged = existingProvider && (existingProvider !== provider || (existingEmbedBaseUrl && existingEmbedBaseUrl !== baseUrl));

if (isLlmChanged) {
  writeFileSync(configPath + '.llm.changed', '1');
}
if (isEmbedChanged) {
  writeFileSync(configPath + '.embed.changed', '1');
}
const existingLlmKey = isLlmChanged ? '' : doc.getIn(['llm', 'apiKey']);
