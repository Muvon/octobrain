"""Thin wrapper around the OpenAI SDK pointed at any OpenAI-compatible endpoint.

Works with: Ollama Cloud, local Ollama (set base to `http://host.docker.internal:11434/v1`),
OpenAI, Together, Groq, Fireworks — anything that speaks `/v1/chat/completions`.

We keep this small on purpose. The SDK already handles retries, backoff, and streaming;
we only need the synchronous chat-completion path for benchmarking.
"""
from __future__ import annotations

import os
from dataclasses import dataclass
from typing import Optional

from openai import OpenAI


@dataclass
class LLMConfig:
    base_url: str
    api_key: str
    model: str

    @classmethod
    def from_env(cls, model_env: str = "AGENT_MODEL") -> "LLMConfig":
        base_url = os.environ.get("AGENT_BASE_URL")
        api_key = os.environ.get("AGENT_API_KEY", "ollama")
        model = os.environ.get(model_env)
        if not base_url:
            raise RuntimeError("AGENT_BASE_URL must be set")
        if not model:
            raise RuntimeError(f"{model_env} must be set")
        return cls(base_url=base_url.rstrip("/"), api_key=api_key, model=model)


class LLMClient:
    """Synchronous OpenAI-compatible chat client."""

    def __init__(self, config: LLMConfig, timeout_s: float = 120.0) -> None:
        self._cfg = config
        self._client = OpenAI(
            base_url=config.base_url,
            api_key=config.api_key,
            timeout=timeout_s,
        )

    @property
    def model(self) -> str:
        return self._cfg.model

    def complete(
        self,
        prompt: str,
        *,
        system: Optional[str] = None,
        temperature: float = 0.0,
        max_tokens: int = 1024,
    ) -> str:
        """Single-turn completion. Returns the assistant message content as plain text."""
        messages: list[dict] = []
        if system:
            messages.append({"role": "system", "content": system})
        messages.append({"role": "user", "content": prompt})

        resp = self._client.chat.completions.create(
            model=self._cfg.model,
            messages=messages,
            temperature=temperature,
            max_tokens=max_tokens,
        )
        choice = resp.choices[0]
        content = choice.message.content or ""
        return content.strip()
