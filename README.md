# agent-profile

`agent-profile` is a proposed local-first profile control plane for multi-agent
engineering workflows.

It helps users define, switch, launch, and govern combinations of:

- agent providers and runtimes
- LLM models
- agent roles
- skills, rules, and prompts
- MCP servers and tools
- permissions and approval modes
- environments and secret sources
- loop topologies and verifier contracts

The first design target is integration with [`agent-loop`](https://github.com/xiaoleiy/agent-loop),
while keeping the profile format portable enough to render into tools such as
Claude Code, Cursor, Codex, Continue, Aider, CrewAI, LangGraph, and MCP
gateways.

## Product Direction

See [`docs/roadmap.md`](docs/roadmap.md) for the roadmap, major feature areas,
and product design principles.

## Status

This repository currently contains the product direction and roadmap. The next
step is to define the initial profile schema and CLI surface.
