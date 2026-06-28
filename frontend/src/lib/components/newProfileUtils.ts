export interface ProfileNameCheck {
  ok: boolean;
  name: string;
  error?: string;
}

/**
 * Validate a new-profile name against the existing profiles.
 * Trims; rejects empty, >48 chars, and case-sensitive duplicates.
 */
export function validateProfileName(raw: string, existing: string[]): ProfileNameCheck {
  const name = raw.trim();

  if (name.length === 0) {
    return { ok: false, name: "", error: "Name required" };
  }

  if (name.length > 48) {
    return { ok: false, name, error: "Name too long (max 48)" };
  }

  if (existing.includes(name)) {
    return { ok: false, name, error: `A profile named "${name}" already exists` };
  }

  return { ok: true, name };
}
