import assert from "node:assert/strict";
import { test } from "node:test";
// @ts-expect-error The checker is a plain script shared with the desktop edition.
import { check } from "../../scripts/i18n.mjs";

test("every interface text is translated, with its placeholders", () => {
  for (const { language, missing, broken } of check()) {
    assert.deepEqual(missing, [], `${language} lacks ${missing.length} translations; run node scripts/i18n.mjs`);
    assert.deepEqual(broken, [], `${language} has translations with different placeholders`);
  }
});
