from typing import List, Dict
from pydantic_ai import Agent
from pydantic_ai.models.anthropic import AnthropicModel
from pydantic_ai.models.groq import GroqModel
from pydantic_ai.models.openai import OpenAIModel
from pydantic_ai.providers.openai import OpenAIProvider
from pydantic_ai.providers.groq import GroqProvider
from pydantic_ai.providers.anthropic import AnthropicProvider

import logging
import os
from dotenv import load_dotenv

try:
    from .db import DatabaseManager
except ImportError:
    from db import DatabaseManager

logger = logging.getLogger(__name__)
load_dotenv()

MAX_TRANSCRIPT_CHARS = 30000
MAX_HISTORY_MESSAGES = 20


class ChatProcessor:
    """Answers free-form questions about a single meeting using the configured LLM provider."""

    def __init__(self):
        self.db = DatabaseManager()

    async def answer(
        self,
        meeting_id: str,
        user_message: str,
        history: List[Dict],
        provider: str,
        model_name: str,
    ) -> str:
        if not meeting_id or not meeting_id.strip():
            raise ValueError("meeting_id is required")
        if not user_message or not user_message.strip():
            raise ValueError("message cannot be empty")
        if not provider or not provider.strip():
            raise ValueError("provider is required")
        if not model_name or not model_name.strip():
            raise ValueError("model is required")

        llm = await self._build_llm(provider, model_name)

        meeting = await self.db.get_meeting(meeting_id)
        if not meeting:
            raise ValueError(f"Meeting {meeting_id} not found")

        transcript_text = self._collect_transcript(meeting)

        system_prompt = self._build_system_prompt(
            meeting_title=meeting.get("title") or "Untitled meeting",
            transcript_text=transcript_text,
        )

        prompt = self._build_prompt(system_prompt, history, user_message)

        agent = Agent(llm, result_retries=1)
        logger.info(
            f"Running chat agent for meeting {meeting_id} with {provider}/{model_name} "
            f"(transcript {len(transcript_text)} chars, history {len(history)} msgs)"
        )
        result = await agent.run(prompt)

        if hasattr(result, "data"):
            answer = result.data
        else:
            answer = result
        return str(answer).strip()

    async def _build_llm(self, provider: str, model_name: str):
        if provider == "claude":
            api_key = await self.db.get_api_key("claude")
            if not api_key:
                raise ValueError("Anthropic API key not set")
            return AnthropicModel(model_name, provider=AnthropicProvider(api_key=api_key))
        if provider == "ollama":
            ollama_host = os.getenv("OLLAMA_HOST", "http://localhost:11434")
            return OpenAIModel(
                model_name=model_name,
                provider=OpenAIProvider(base_url=f"{ollama_host}/v1"),
            )
        if provider == "groq":
            api_key = await self.db.get_api_key("groq")
            if not api_key:
                raise ValueError("Groq API key not set")
            return GroqModel(model_name, provider=GroqProvider(api_key=api_key))
        if provider == "openai":
            api_key = await self.db.get_api_key("openai")
            if not api_key:
                raise ValueError("OpenAI API key not set")
            return OpenAIModel(model_name, provider=OpenAIProvider(api_key=api_key))
        raise ValueError(f"Unsupported model provider: {provider}")

    def _collect_transcript(self, meeting: dict) -> str:
        transcripts = meeting.get("transcripts") or []
        text = "\n".join(t.get("text", "") for t in transcripts if t.get("text"))
        if len(text) > MAX_TRANSCRIPT_CHARS:
            text = text[:MAX_TRANSCRIPT_CHARS] + "\n\n[transcript truncated for length]"
        return text

    def _build_system_prompt(self, meeting_title: str, transcript_text: str) -> str:
        sections = [
            "You are a helpful assistant answering questions about a recorded meeting.",
            "Ground every answer strictly in the meeting transcript below. "
            "Quote only verbatim text that actually appears in the transcript. "
            "If the answer is not in the transcript, say you cannot find it rather than guessing.",
            "Keep answers concise and reference specific moments or speakers when relevant.",
            f"Meeting title: {meeting_title}",
            "--- TRANSCRIPT ---",
            transcript_text or "(no transcript available)",
            "--- END TRANSCRIPT ---",
        ]
        return "\n\n".join(sections)

    def _build_prompt(self, system_prompt: str, history: List[Dict], user_message: str) -> str:
        recent = history[-MAX_HISTORY_MESSAGES:] if history else []
        lines = [system_prompt, "", "Conversation so far:"]
        if not recent:
            lines.append("(no prior messages)")
        else:
            for msg in recent:
                role = "User" if msg.get("role") == "user" else "Assistant"
                lines.append(f"{role}: {msg.get('content', '')}")
        lines.append(f"User: {user_message}")
        lines.append("Assistant:")
        return "\n".join(lines)
