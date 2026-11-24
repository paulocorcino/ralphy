// The git name and email the Add a project dialog asks for when git on the
// chosen computer has none, so the first commit of a new repository can be made.
//
// PURE, as `wb-add-project.ts`. The rules match `valid_git_name` and
// `valid_git_email` in the daemon, which checks them again.

/** The name and email typed, and the place whose git settings get them. */
export type Identity = { name: string; email: string; place: string };

/** The longest name or email the daemon takes, in UTF-8 bytes. */
const MAX_BYTES = 256;

// A name git can save: not empty, not too long, no control character, and no
// leading `-`. The value is trimmed first, as the payload sends it.
export function validName(value: string) {
  const v = value.trim();
  return (
    v !== "" &&
    new TextEncoder().encode(v).length <= MAX_BYTES &&
    !v.startsWith("-") &&
    !/[\u0000-\u001F\u007F-\u009F]/u.test(v)
  );
}

// An email git can save: a valid name with text on both sides of an `@`, and
// no space or angle bracket.
export function validEmail(value: string) {
  const v = value.trim();
  const at = v.indexOf("@");
  return validName(v) && !/[\s<>]/u.test(v) && at > 0 && at < v.length - 1;
}

// Both fields are valid.
export function identityReady(identity: Identity) {
  return validName(identity.name) && validEmail(identity.email);
}

// The line that says why the dialog asks.
export function identityHelp(identity: Identity) {
  return (
    `Git on ${identity.place} has no name and email for commits. ` +
    "Ralphy saves them in the git settings of that computer."
  );
}

// The two `project.add` params, trimmed.
export function identityParams(identity: Identity) {
  return { gitName: identity.name.trim(), gitEmail: identity.email.trim() };
}
