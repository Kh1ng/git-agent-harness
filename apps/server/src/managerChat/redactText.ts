/** Scrub common credential shapes before text crosses a model or messaging boundary. */
export function redactTextSecrets(value: string): string {
  return value
    .replace(/\b(?:gh[pousr]_[A-Za-z0-9_]{20,}|github_pat_[A-Za-z0-9_]{20,})\b/g, '[REDACTED:GITHUB_TOKEN]')
    .replace(/\bglpat-[A-Za-z0-9_-]{20,}\b/g, '[REDACTED:GITLAB_TOKEN]')
    .replace(/\bsk-[A-Za-z0-9_-]{20,}\b/g, '[REDACTED:API_KEY]')
    .replace(/(authorization\s*:\s*bearer\s+)[^\s"']+/gi, '$1[REDACTED:TOKEN]')
    .replace(/(https?:\/\/)[^/\s@]+@/gi, '$1[REDACTED:URL_CREDENTIAL]@')
    .replace(/([?&](?:access_token|api[_-]?key|token|password)=)[^&#\s]+/gi, '$1[REDACTED:URL_CREDENTIAL]')
    .replace(/((?:api[_-]?key|token|secret|password)\s*[=:]\s*)[^\s]+/gi, '$1[REDACTED:SECRET]');
}
