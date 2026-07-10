import { useEffect, useState } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { Input } from './ui/input';
import { Button } from './ui/button';
import { Label } from './ui/label';
import { Eye, EyeOff, Loader2 } from 'lucide-react';
import { toast } from 'sonner';

/** One masked secret row persisted via the transcript API-key commands. */
function SecretField({
    provider,
    placeholder,
    testCommand,
}: {
    provider: string;
    placeholder: string;
    testCommand?: string;
}) {
    const [value, setValue] = useState('');
    const [savedValue, setSavedValue] = useState('');
    const [show, setShow] = useState(false);
    const [isBusy, setIsBusy] = useState(false);

    useEffect(() => {
        invoke<string>('api_get_transcript_api_key', { provider })
            .then((key) => {
                setValue(key || '');
                setSavedValue(key || '');
            })
            .catch((err) => console.error(`Failed to load ${provider} key:`, err));
    }, [provider]);

    const isDirty = value.trim() !== savedValue.trim();

    const handleSave = async () => {
        const trimmed = value.trim();
        setIsBusy(true);
        try {
            if (trimmed) {
                await invoke('api_save_transcript_api_key', { provider, apiKey: trimmed });
            } else {
                await invoke('api_delete_transcript_api_key', { provider });
            }
            setSavedValue(trimmed);
            toast.success(trimmed ? 'Saved' : 'Removed');
        } catch (err) {
            console.error(`Failed to save ${provider} key:`, err);
            toast.error('Failed to save', {
                description: err instanceof Error ? err.message : String(err),
            });
        } finally {
            setIsBusy(false);
        }
    };

    const handleTest = async () => {
        if (!testCommand || !value.trim()) return;
        setIsBusy(true);
        try {
            await invoke(testCommand, { apiKey: value.trim() });
            toast.success('Key is valid');
        } catch (err) {
            toast.error('Key check failed', {
                description:
                    typeof err === 'string' ? err : err instanceof Error ? err.message : String(err),
            });
        } finally {
            setIsBusy(false);
        }
    };

    return (
        <div>
            <div className="relative mx-1">
                <Input
                    type={show ? 'text' : 'password'}
                    className="pr-12 focus:ring-1 focus:ring-ring focus:border-ring"
                    value={value}
                    onChange={(e) => setValue(e.target.value)}
                    placeholder={placeholder}
                />
                <div className="absolute inset-y-0 right-0 pr-1 flex items-center">
                    <Button type="button" variant="ghost" size="icon" onClick={() => setShow(!show)}>
                        {show ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                    </Button>
                </div>
            </div>
            <div className="flex gap-2 mt-2 mx-1">
                <Button
                    type="button"
                    size="sm"
                    onClick={handleSave}
                    disabled={isBusy || !isDirty}
                    className="bg-primary text-primary-foreground hover:bg-brand-hover"
                >
                    {isBusy ? <Loader2 className="h-4 w-4 mr-1 animate-spin" /> : null}
                    Save
                </Button>
                {testCommand && (
                    <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        onClick={handleTest}
                        disabled={isBusy || !value.trim()}
                    >
                        Test key
                    </Button>
                )}
            </div>
        </div>
    );
}

/**
 * Speaker identification (diarization) settings — self-contained section
 * rendered inside the Transcript settings tab.
 *
 * - pyannoteAI API key → unlocks the cloud option (best accuracy).
 * - Hugging Face token → unlocks "Local Pro" (pyannote community-1 in a local
 *   Python sidecar; the model is gated so each user needs their own token).
 */
export function DiarizationSettings() {
    return (
        <div className="border-t border-border pt-4 mt-2 space-y-5">
            <div>
                <Label className="block text-sm font-medium text-muted-foreground mb-1">
                    Speaker identification (diarization)
                </Label>
                <p className="text-xs text-muted-foreground">
                    Runs on-device by default. The keys below unlock the higher-accuracy
                    options in the &quot;Identify speakers&quot; dialog — both are optional.
                </p>
            </div>

            <div>
                <Label className="block text-sm font-medium text-muted-foreground mb-1">
                    pyannoteAI API key <span className="font-normal text-muted-foreground">(cloud, best accuracy)</span>
                </Label>
                <SecretField
                    provider="pyannote"
                    placeholder="pyannoteAI API key"
                    testCommand="api_test_pyannote_key"
                />
            </div>

            <div>
                <Label className="block text-sm font-medium text-muted-foreground mb-1">
                    Hugging Face token <span className="font-normal text-muted-foreground">(Local Pro, fully private)</span>
                </Label>
                <p className="text-xs text-muted-foreground mb-2">
                    Local Pro runs the pyannote community-1 model on this machine (first
                    use downloads ~1–2 GB). The model is gated: create a free Hugging Face
                    account, accept the conditions on the{' '}
                    <button
                        type="button"
                        className="text-brand hover:underline"
                        onClick={() =>
                            invoke('open_external_url', {
                                url: 'https://huggingface.co/pyannote/speaker-diarization-community-1',
                            }).catch(() => {})
                        }
                    >
                        model page
                    </button>{' '}
                    and paste a read-access token here.
                </p>
                <SecretField provider="huggingface" placeholder="hf_…" />
            </div>
        </div>
    );
}
