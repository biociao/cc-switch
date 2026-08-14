import { fireEvent, render, screen, waitFor } from "@testing-library/react";
import { useState, type ComponentProps, type PropsWithChildren } from "react";
import { useForm } from "react-hook-form";
import { describe, expect, it, vi } from "vitest";
import { CodexFormFields } from "@/components/providers/forms/CodexFormFields";
import { normalizeCodexCatalogModelsForSave } from "@/components/providers/forms/ProviderForm";
import { Form } from "@/components/ui/form";
import type { CodexCatalogModel } from "@/types";

type CodexFormFieldsProps = ComponentProps<typeof CodexFormFields>;

const FormShell = ({ children }: PropsWithChildren) => {
  const form = useForm();
  return <Form {...form}>{children}</Form>;
};

// 模拟 ProviderForm：catalogModels 存在父组件 state 中，子组件通过回调回传
const StatefulHarness = ({
  initialCatalog,
  onSnapshot,
}: {
  initialCatalog: CodexCatalogModel[];
  onSnapshot: (models: CodexCatalogModel[]) => void;
}) => {
  const [catalogModels, setCatalogModels] = useState(initialCatalog);

  const props: CodexFormFieldsProps = {
    codexApiKey: "sk-test",
    onApiKeyChange: vi.fn(),
    category: "third_party",
    shouldShowApiKeyLink: false,
    websiteUrl: "",
    shouldShowSpeedTest: false,
    codexBaseUrl: "https://opencode.ai/zen/go/v1",
    onBaseUrlChange: vi.fn(),
    isFullUrl: false,
    onFullUrlChange: vi.fn(),
    isEndpointModalOpen: false,
    onEndpointModalToggle: vi.fn(),
    autoSelect: false,
    onAutoSelectChange: vi.fn(),
    codexModel: "glm-5.2",
    onModelChange: vi.fn(),
    apiFormat: "openai_responses",
    onApiFormatChange: vi.fn(),
    anthropicAuthField: "ANTHROPIC_AUTH_TOKEN",
    onAnthropicAuthFieldChange: vi.fn(),
    impersonateClaudeCode: false,
    onImpersonateClaudeCodeChange: vi.fn(),
    maxOutputTokens: "",
    onMaxOutputTokensChange: vi.fn(),
    promptCacheRouting: "auto",
    onPromptCacheRoutingChange: vi.fn(),
    catalogModels,
    onCatalogModelsChange: (next) => {
      setCatalogModels(next);
      onSnapshot(next);
    },
    speedTestEndpoints: [],
    customUserAgent: "",
    onCustomUserAgentChange: vi.fn(),
    localProxyHeadersOverride: "",
    onLocalProxyHeadersOverrideChange: vi.fn(),
    localProxyBodyOverride: "",
    onLocalProxyBodyOverrideChange: vi.fn(),
  };

  return (
    <FormShell>
      <CodexFormFields {...props} />
    </FormShell>
  );
};

const DB_CATALOG: CodexCatalogModel[] = [
  { model: "glm-5.2", displayName: "GLM 5.2", contextWindow: 204800 },
  { model: "glm-5.1", displayName: "GLM 5.1", contextWindow: 204800 },
];

describe("CodexFormFields 模型映射编辑", () => {
  it("添加一行并填写后，父组件收到包含新行的完整列表", async () => {
    let latest: CodexCatalogModel[] = DB_CATALOG;
    render(
      <StatefulHarness
        initialCatalog={DB_CATALOG}
        onSnapshot={(models) => {
          latest = models;
        }}
      />,
    );

    // 已有两行渲染出来（默认模型输入框也是 glm-5.2，故用 getAll）
    expect(
      screen.getAllByDisplayValue("glm-5.2").length,
    ).toBeGreaterThanOrEqual(2);

    fireEvent.click(screen.getByRole("button", { name: "添加模型" }));

    // 新行的输入框（placeholder 识别）
    const modelInputs = await screen.findAllByPlaceholderText(
      "例如: deepseek-v4-flash",
    );
    const displayInputs = screen.getAllByPlaceholderText(
      "例如: DeepSeek V4 Flash",
    );
    expect(modelInputs).toHaveLength(3);
    expect(displayInputs).toHaveLength(3);

    fireEvent.change(displayInputs[2], { target: { value: "GPT-5.2" } });
    fireEvent.change(modelInputs[2], { target: { value: "kimi-k2.7-code" } });

    await waitFor(() => {
      expect(latest).toHaveLength(3);
    });
    expect(latest[2]).toMatchObject({
      model: "kimi-k2.7-code",
      displayName: "GPT-5.2",
    });

    // 保存时 normalize 后仍保留新行
    const saved = normalizeCodexCatalogModelsForSave(latest);
    expect(saved.map((m) => m.model)).toEqual([
      "glm-5.2",
      "glm-5.1",
      "kimi-k2.7-code",
    ]);
  });

  it("修改已有行的显示名会回传到父组件", async () => {
    let latest: CodexCatalogModel[] = DB_CATALOG;
    render(
      <StatefulHarness
        initialCatalog={DB_CATALOG}
        onSnapshot={(models) => {
          latest = models;
        }}
      />,
    );

    const displayInputs = screen.getAllByPlaceholderText(
      "例如: DeepSeek V4 Flash",
    );
    fireEvent.change(displayInputs[0], { target: { value: "GLM 5.2 Pro" } });

    await waitFor(() => {
      expect(latest[0].displayName).toBe("GLM 5.2 Pro");
    });
    expect(latest).toHaveLength(2);
  });
});
