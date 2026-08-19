'use client';

import { ArrowRight } from 'lucide-react';
import { Button } from './ui/button';
import { SummaryLanguageSettings } from '@/components/SummaryLanguageSettings';
import { Switch } from './ui/switch';
import { useConfig } from '@/contexts/ConfigContext';

interface SummaryModelSettingsProps {
  /** Switches the settings page to the AI Providers tab. */
  onOpenProviderSettings?: () => void;
}

export function SummaryModelSettings({ onOpenProviderSettings }: SummaryModelSettingsProps) {
  const { isAutoSummary, toggleIsAutoSummary } = useConfig();

  return (
    <div className='flex flex-col gap-4'>
      <div className="bg-card rounded-lg border border-border p-6">
        <div className="flex items-center justify-between">
          <div>
            <h3 className="text-lg font-semibold text-foreground mb-2">Auto Summary</h3>
            <p className="text-sm text-muted-foreground">Auto Generating summary after meeting completion(Stopping)</p>
          </div>
          <Switch checked={isAutoSummary} onCheckedChange={toggleIsAutoSummary} />
        </div>
      </div>

      <SummaryLanguageSettings />

      <div className="bg-card rounded-lg border border-border p-6">
        <h3 className="text-lg font-semibold text-foreground mb-2">AI Model</h3>
        <p className="text-sm text-muted-foreground">
          Summaries use the app-wide AI provider, shared with meeting chat and live Ask AI.
        </p>
        {onOpenProviderSettings && (
          <Button variant="outline" size="sm" className="mt-4" onClick={onOpenProviderSettings}>
            Configure in AI Providers
            <ArrowRight className="ml-2 h-4 w-4" />
          </Button>
        )}
      </div>
    </div>
  );
}
