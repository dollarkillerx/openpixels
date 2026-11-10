/** `name`, or `name (2)`, `name (3)`… before the extension, whichever is not taken. */
export function freeName(name: string, taken: Set<string>): string {
  const dot = name.lastIndexOf(".");
  const [base, extension] = dot > 0 ? [name.slice(0, dot), name.slice(dot)] : [name, ""];
  let candidate = name;
  for (let n = 2; taken.has(candidate.toLowerCase()); n++) candidate = `${base} (${n})${extension}`;
  return candidate;
}
