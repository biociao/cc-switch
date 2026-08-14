import { describe, expect, it } from "vitest";
import {
  codexAggregateTemplateRows,
  CODEX_OFFICIAL_MODEL_SUGGESTIONS,
  rowsToCustomRoutes,
  validateAggregateRoutes,
} from "@/utils/aggregateRoutes";

describe("codex 聚合路由预填模板行", () => {
  it("模板行覆盖全部官方模型建议，且带 template 标记", () => {
    const rows = codexAggregateTemplateRows();
    expect(rows.map((row) => row.key)).toEqual(
      CODEX_OFFICIAL_MODEL_SUGGESTIONS.map((suggestion) => suggestion.id),
    );
    expect(
      rows.every(
        (row) => row.template === true && !row.providerId && !row.model,
      ),
    ).toBe(true);
  });

  it("未动过的模板行跳过 incomplete 校验（全部未配置时报 empty）", () => {
    const rows = codexAggregateTemplateRows();
    const validation = validateAggregateRoutes(
      { custom: rowsToCustomRoutes(rows) },
      "codex",
      rows,
    );
    expect(validation).toEqual({ ok: false, reason: "empty" });
  });

  it("模板行补全供应商与模型后正常通过校验，未配置的模板行不落入结果", () => {
    const rows = codexAggregateTemplateRows();
    // 用户配置第一行：UI 层 patchRow 会清掉 template 标记，这里模拟清掉后的形态
    const { template: _template, ...first } = rows[0];
    rows[0] = { ...first, providerId: "kimi", model: "k2" };
    const validation = validateAggregateRoutes(
      { custom: rowsToCustomRoutes(rows) },
      "codex",
      rows,
    );
    expect(validation).toEqual({
      ok: true,
      routes: { custom: { "gpt-5.6": { providerId: "kimi", model: "k2" } } },
    });
  });

  it("用户碰过但只填了一半的行仍报 incomplete", () => {
    const rows = codexAggregateTemplateRows();
    const { template: _template, ...first } = rows[0];
    rows[0] = { ...first, providerId: "kimi" };
    const validation = validateAggregateRoutes(
      { custom: rowsToCustomRoutes(rows) },
      "codex",
      rows,
    );
    expect(validation).toEqual({
      ok: false,
      reason: "incomplete",
      tier: "gpt-5.6",
    });
  });

  it("手动加的 key-only 行（非模板）仍报 incomplete", () => {
    const rows = [
      ...codexAggregateTemplateRows(),
      { key: "my-model", providerId: "", model: "" },
    ];
    const validation = validateAggregateRoutes(
      { custom: rowsToCustomRoutes(rows) },
      "codex",
      rows,
    );
    expect(validation).toEqual({
      ok: false,
      reason: "incomplete",
      tier: "my-model",
    });
  });
});
