import { describe, expect, it } from "vitest";
import { renderTemplate } from "./render";

describe("renderTemplate", () => {
  it("fills placeholders and escapes html", () => {
    const html = renderTemplate("receipt", { body: "<b>hi</b>" });
    expect(html).toBe("<h1>Receipt</h1><p>&lt;b&gt;hi&lt;/b&gt;</p>");
  });

  it("throws for unknown templates", () => {
    expect(() => renderTemplate("nope", {})).toThrow("Unknown template");
  });
});
