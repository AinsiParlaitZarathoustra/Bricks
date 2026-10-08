"""
cersei-tbench agent adapter for terminal-bench 2.0 (harbor framework).

A purpose-built Terminal-Bench coding agent on the Cersei SDK + AgentRL —
distinct from the general `abstract` CLI. Uploads a prebuilt static `tbench-agent`
binary and runs it with a TB-tuned system prompt.

Usage:
    cd bench/term-bench && uv sync
    GOOGLE_API_KEY=<key> PYTHONPATH=. uv run harbor run \
        --agent-import-path tbench_agent:CerseiTBenchAgent \
        --model google/gemini-3.1-pro-preview \
        --dataset terminal-bench@2.0 \
        --n-concurrent 8

Tuning (host env, forwarded as flags):
    TBENCH_SAMPLES   best-of-N attempts (default 1; helps only with a real test script)
    TBENCH_ROUNDS    recovery rounds (default 0; helps only with a real test script)
"""

import os
import shlex
from pathlib import Path

from harbor.agents.installed.base import (  # type:ignore
    BaseInstalledAgent,
    EnvVar,
    with_prompt_template,
)
from harbor.environments.base import BaseEnvironment  # type:ignore
from harbor.models.agent.context import AgentContext  # type:ignore
from harbor.models.trial.paths import EnvironmentPaths  # type:ignore

_BENCH_DIR = Path(__file__).resolve().parent
_BINARY_ARM64 = _BENCH_DIR / "tbench-agent-linux-arm64"
_BINARY_AMD64 = _BENCH_DIR / "tbench-agent-linux-amd64"

# Vertex service-account key (host path). Forwarded into each ephemeral container
# so the agent can mint self-refreshing access tokens. Override with VERTEX_SA_FILE.
_SA_FILE = Path(os.environ.get("VERTEX_SA_FILE", _BENCH_DIR.parent.parent / "service-account.json"))
_SA_CONTAINER_PATH = "/tmp/vertex-sa.json"


class CerseiTBenchAgent(BaseInstalledAgent):
    """Purpose-built Terminal-Bench agent on the Cersei SDK + AgentRL."""

    ENV_VARS = [
        EnvVar("google_api_key", env="GOOGLE_API_KEY", env_fallback="GOOGLE_API_KEY"),
        EnvVar("gemini_api_key", env="GEMINI_API_KEY", env_fallback="GEMINI_API_KEY"),
        EnvVar("anthropic_api_key", env="ANTHROPIC_API_KEY", env_fallback="ANTHROPIC_API_KEY"),
        EnvVar("openai_api_key", env="OPENAI_API_KEY", env_fallback="OPENAI_API_KEY"),
    ]

    @staticmethod
    def name() -> str:
        return "cersei-tbench"

    def get_version_command(self) -> str | None:
        return "tbench-agent --version"

    def parse_version(self, stdout: str) -> str:
        return stdout.strip().removeprefix("tbench-agent").strip()

    async def install(self, environment: BaseEnvironment) -> None:
        result = await environment.exec(command="uname -m")
        arch = result.stdout.strip() if result.stdout else ""
        binary_path = _BINARY_AMD64 if ("x86_64" in arch or "amd64" in arch) else _BINARY_ARM64
        if not binary_path.exists():
            raise RuntimeError(
                f"Binary not found at {binary_path}. Build with the musl cross-toolchain "
                "(see project_agentrl_termbench memory / TERMINAL_BENCH.md)."
            )
        await environment.upload_file(
            source_path=binary_path,
            target_path="/usr/local/bin/tbench-agent",
        )
        await self.exec_as_root(environment, command="chmod +x /usr/local/bin/tbench-agent")

        # Carry the Vertex service-account key into the (ephemeral) container so
        # the agent can mint self-refreshing tokens. Lands in /tmp (never /output).
        if _SA_FILE.is_file():
            await environment.upload_file(
                source_path=_SA_FILE,
                target_path=_SA_CONTAINER_PATH,
            )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        escaped = shlex.quote(instruction)
        model_flag = f"--model {self.model_name} " if self.model_name else ""
        samples = os.environ.get("TBENCH_SAMPLES", "1")
        rounds = os.environ.get("TBENCH_ROUNDS", "0")

        env = self.resolve_env_vars()

        # Vertex config: project/location + point the agent at the uploaded SA key.
        env["VERTEX_PROJECT_ID"] = os.environ.get("VERTEX_PROJECT_ID", "")
        env["VERTEX_LOCATION"] = os.environ.get("VERTEX_LOCATION", "global")
        if _SA_FILE.is_file():
            env["GOOGLE_APPLICATION_CREDENTIALS"] = _SA_CONTAINER_PATH

        # Inject learned failure patterns as extra hints.
        patterns_file = _BENCH_DIR / "failure_patterns.txt"
        if patterns_file.exists():
            active = [
                ln.strip()
                for ln in patterns_file.read_text().splitlines()
                if ln.strip() and not ln.strip().startswith("#")
            ][:20]
            if active:
                env["TBENCH_HINTS"] = "\n".join(active)

        output_path = EnvironmentPaths.agent_dir / "tbench-output.jsonl"

        await self.exec_as_agent(
            environment,
            command=(
                f"tbench-agent -p {escaped} "
                f"{model_flag}"
                f"--samples {shlex.quote(samples)} "
                f"--rounds {shlex.quote(rounds)} "
                f"--json "
                f"2>&1 | tee {output_path}"
            ),
            env=env,
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        """tbench-agent does not stream token/cost; nothing to extract."""
        return None
