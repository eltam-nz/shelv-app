/**
 * The tag palette.
 *
 * Tags store a token such as `pastel-blue`, never a raw colour. That is what
 * lets the theme guarantee every tag stays readable: the tokens here are the
 * only colours a tag can take, and scripts/check-contrast.mjs verifies each
 * one against the chip background it actually renders on.
 */

/** Every colour a tag may use. */
export const TAG_TOKENS = [
  "pastel-blue",
  "pastel-mint",
  "pastel-rose",
  "pastel-amber",
  "pastel-lilac",
  "pastel-teal",
  "pastel-peach",
  "pastel-sage",
] as const;

/** A palette token. */
export type TagToken = (typeof TAG_TOKENS)[number];

/** The CSS custom properties a token maps to. */
interface TokenStyle {
  /** Text colour. */
  color: string;
  /** Chip background. */
  backgroundColor: string;
}

const STYLES: Record<TagToken, TokenStyle> = {
  "pastel-blue": {
    color: "var(--accent-blue)",
    backgroundColor: "var(--accent-blue-fill)",
  },
  "pastel-mint": {
    color: "var(--accent-mint)",
    backgroundColor: "var(--accent-mint-fill)",
  },
  "pastel-rose": {
    color: "var(--accent-rose)",
    backgroundColor: "var(--accent-rose-fill)",
  },
  "pastel-amber": {
    color: "var(--accent-amber)",
    backgroundColor: "var(--accent-amber-fill)",
  },
  "pastel-lilac": {
    color: "var(--accent-lilac)",
    backgroundColor: "var(--accent-lilac-fill)",
  },
  "pastel-teal": {
    color: "var(--accent-teal)",
    backgroundColor: "var(--accent-teal-fill)",
  },
  "pastel-peach": {
    color: "var(--accent-peach)",
    backgroundColor: "var(--accent-peach-fill)",
  },
  "pastel-sage": {
    color: "var(--accent-sage)",
    backgroundColor: "var(--accent-sage-fill)",
  },
};

/** Whether a stored string is a token this build knows. */
export function isTagToken(value: string): value is TagToken {
  return (TAG_TOKENS as readonly string[]).includes(value);
}

/**
 * Resolves a stored colour token to inline styles.
 *
 * An unrecognised token — a tag written by a newer build, or a hand-edited
 * database — falls back to the neutral style rather than to an arbitrary
 * colour, so it stays readable instead of becoming invisible.
 */
export function tagStyle(token: string): TokenStyle {
  return isTagToken(token)
    ? STYLES[token]
    : { color: "var(--fg-muted)", backgroundColor: "var(--surface-raised)" };
}
