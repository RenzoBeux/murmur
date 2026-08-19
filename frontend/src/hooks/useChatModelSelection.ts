import { useCallback, useMemo, useRef, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { configService, ModelConfig } from '@/services/configService';
import { ChatProvider, PROVIDER_LABEL } from '@/components/chat/ModelPicker';

/**
 * The chat model selection shared by the meeting-details ChatPanel and the
 * live Ask-AI panel: reads the app-wide model config, persists a pick through
 * `api_save_model_config`, and broadcasts it via the `model-config-updated`
 * event so every other consumer (summary, other panels) follows.
 */
export function useChatModelSelection() {
  const { modelConfig, setModelConfig, models, modelOptions, providerApiKeys } = useConfig();

  const provider = (modelConfig.provider as ChatProvider) || 'ollama';
  const model = modelConfig.model || '';

  const ollamaModelNames = useMemo(() => models.map((m) => m.name), [models]);

  // Options for the providers the ConfigContext doesn't track live (LM Studio,
  // OpenRouter, ChatGPT subscription, custom OpenAI). Fetched once, the first
  // time the picker opens, so merely mounting a chat panel never probes
  // LM Studio, openrouter.ai, or the ChatGPT token store in the background.
  const [lmStudioModels, setLmStudioModels] = useState<string[]>([]);
  const [openRouterModels, setOpenRouterModels] = useState<string[]>([]);
  const [chatgptModels, setChatgptModels] = useState<string[]>([]);
  const [chatgptSignedIn, setChatgptSignedIn] = useState(false);
  const [customOpenAIModel, setCustomOpenAIModel] = useState<string>('');
  const extrasLoaded = useRef(false);

  const handlePickerOpen = useCallback(() => {
    if (extrasLoaded.current) return;
    extrasLoaded.current = true;
    // Each probe fails independently and silently: a dead LM Studio or a
    // missing ChatGPT sign-in just leaves that group empty or disabled.
    invoke<{ id: string; name: string }[]>('get_lmstudio_models', {
      endpoint: modelConfig.lmStudioEndpoint ?? null,
    })
      .then((list) => setLmStudioModels(list.map((m) => m.name)))
      .catch(() => {});
    invoke<{ signed_in: boolean }>('chatgpt_status')
      .then((s) => setChatgptSignedIn(!!s?.signed_in))
      .catch(() => {});
    invoke<string[]>('chatgpt_list_models')
      .then((list) => {
        if (list?.length) setChatgptModels(list);
      })
      .catch(() => {});
    configService
      .getCustomOpenAIConfig()
      .then((cfg) => setCustomOpenAIModel(cfg?.model ?? ''))
      .catch(() => {});
    if (providerApiKeys.openrouter) {
      invoke<{ id: string }[]>('get_openrouter_models')
        .then((list) => setOpenRouterModels(list.map((m) => m.id).sort()))
        .catch(() => {});
    }
  }, [modelConfig.lmStudioEndpoint, providerApiKeys.openrouter]);

  const pickerModelOptions: Record<string, string[]> = useMemo(
    () => ({
      ...modelOptions,
      lmstudio: lmStudioModels,
      openrouter: openRouterModels,
      'custom-openai': customOpenAIModel ? [customOpenAIModel] : [],
      'chatgpt-subscription':
        chatgptModels.length > 0 ? chatgptModels : modelOptions['chatgpt-subscription'],
    }),
    [modelOptions, lmStudioModels, openRouterModels, customOpenAIModel, chatgptModels]
  );

  const persistModelChange = async (next: ModelConfig) => {
    try {
      await invoke('api_save_model_config', {
        provider: next.provider,
        model: next.model,
        whisperModel: next.whisperModel,
        apiKey: next.apiKey ?? null,
        ollamaEndpoint: next.ollamaEndpoint ?? null,
        lmStudioEndpoint: next.lmStudioEndpoint ?? null,
      });
      const { emit } = await import('@tauri-apps/api/event');
      await emit('model-config-updated', next);
    } catch (err) {
      console.error('Failed to save model config:', err);
      toast.error('Failed to save model selection');
    }
  };

  const handlePickModel = async (nextProvider: ChatProvider, nextModel: string) => {
    const requiresKey: ChatProvider[] = ['claude', 'groq', 'openai', 'openrouter'];
    if (requiresKey.includes(nextProvider)) {
      const key = providerApiKeys[nextProvider as keyof typeof providerApiKeys];
      if (!key) {
        toast.error(`No API key for ${PROVIDER_LABEL[nextProvider]}. Add one in Settings first.`);
        return;
      }
    }
    if (nextProvider === 'chatgpt-subscription' && !chatgptSignedIn) {
      toast.error('Sign in with ChatGPT in Settings first.');
      return;
    }
    const next: ModelConfig = {
      ...modelConfig,
      provider: nextProvider,
      model: nextModel,
    };
    setModelConfig(next);
    await persistModelChange(next);
  };

  return {
    provider,
    model,
    ollamaModelNames,
    modelOptions: pickerModelOptions,
    providerApiKeys,
    chatgptSignedIn,
    onPickerOpen: handlePickerOpen,
    handlePickModel,
  };
}
