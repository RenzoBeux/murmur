import { useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { toast } from 'sonner';
import { useConfig } from '@/contexts/ConfigContext';
import { ModelConfig } from '@/services/configService';
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
    modelOptions,
    providerApiKeys,
    handlePickModel,
  };
}
