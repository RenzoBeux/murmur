import React from 'react';
import { Lock, Sparkles, Cpu } from 'lucide-react';
import { Button } from '@/components/ui/button';
import { OnboardingContainer } from '../OnboardingContainer';
import { useOnboarding } from '@/contexts/OnboardingContext';
import { Logomark } from '@/components/brand/Logomark';

export function WelcomeStep() {
  const { goNext } = useOnboarding();

  const features = [
    {
      icon: Lock,
      title: 'Your data never leaves your device',
    },
    {
      icon: Sparkles,
      title: 'Intelligent summaries & insights',
    },
    {
      icon: Cpu,
      title: 'Works offline, no cloud required',
    },
  ];

  return (
    <OnboardingContainer
      title="Welcome to Meetily"
      description="Record. Transcribe. Summarize. All on your device."
      step={1}
      hideProgress={true}
    >
      <div className="flex flex-col items-center space-y-10">
        {/* Logomark */}
        <Logomark size={72} className="animate-fade-in-up" />

        {/* Divider */}
        <div className="w-16 h-px bg-border" />

        {/* Features Card */}
        <div className="w-full max-w-md bg-card rounded-lg border border-border p-6 space-y-4">
          {features.map((feature, index) => {
            const Icon = feature.icon;
            return (
              <div key={index} className="flex items-start gap-3">
                <div className="flex-shrink-0 mt-0.5">
                  <div className="w-5 h-5 rounded-full bg-brand/10 flex items-center justify-center">
                    <Icon className="w-3 h-3 text-brand" />
                  </div>
                </div>
                <p className="text-sm text-muted-foreground leading-relaxed">{feature.title}</p>
              </div>
            );
          })}
        </div>

        {/* CTA Section */}
        <div className="w-full max-w-xs space-y-3">
          <Button
            onClick={goNext}
            className="w-full h-11 bg-primary text-primary-foreground hover:bg-brand-hover shadow-glow"
          >
            Get Started
          </Button>
          <p className="text-xs text-center text-muted-foreground">Takes less than 3 minutes</p>
        </div>
      </div>
    </OnboardingContainer>
  );
}
