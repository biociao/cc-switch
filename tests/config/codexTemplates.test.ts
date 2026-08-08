import { describe, expect, it } from "vitest";
import { parse as parseToml } from "smol-toml";
import { getCodexCustomTemplate } from "@/config/codexTemplates";

describe("Codex custom templates", () => {
  it("does not force Codex Goal mode in the custom provider template", () => {
    const template = getCodexCustomTemplate();
    const parsed = parseToml(template.config) as {
      features?: { goals?: boolean };
      model_providers?: Record<string, unknown>;
    };

    expect(template.auth).toEqual({ OPENAI_API_KEY: "" });
    expect(parsed.features?.goals).toBeUndefined();
    expect(parsed.model_providers?.custom).toBeDefined();
  });

  it("does not force ChatGPT OAuth on the third-party custom provider", () => {
    // Codex 0.144+ treats `requires_openai_auth = true` as an explicit
    // ChatGPT OAuth requirement, which hijacks third-party API-key auth.
    const template = getCodexCustomTemplate();
    const parsed = parseToml(template.config) as {
      model_providers?: { custom?: { requires_openai_auth?: boolean } };
    };

    expect(
      parsed.model_providers?.custom?.requires_openai_auth,
    ).toBeUndefined();
    expect(template.config).not.toContain("requires_openai_auth");
  });
});
