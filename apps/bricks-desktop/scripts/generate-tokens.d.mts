export interface GenerateTokensOptions {
  check?: boolean;
}

/** Generate CSS files from src/lib/theme/tokens.json or report stale paths in check mode. */
export function generateTokens(options?: GenerateTokensOptions): string[];
