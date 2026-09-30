---
title: HydraFusion in VS Code and the GitHub Copilot app
link: https://github.blog/changelog/2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app
source: github-changelog
published: 2026-09-30T14:31:19Z
updated: 2026-09-30T14:31:19Z
first_seen: 2026-09-30T17:56:37.314048Z
authors:
- Allison
labels:
- copilot
- release
summary: The HydraFusion research preview is now available in Visual Studio Code and the GitHub Copilot app, expanding beyond Copilot CLI. HydraFusion appears in the model picker, but rather than being… The post HydraFusion in VS Code and the GitHub Copilot app appeared first on The GitHub Blog.
content: extracted
html: 2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app.html
preview:
  file: 2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app.preview-87ffde0439c8.webp
  width: 256
  height: 135
  color: '#101e1a'
images:
- source: https://github.blog/wp-content/themes/github-2021-child/dist/img/social-v3-new-releases.jpg
  original:
    file: 2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app.image-a6211ea9e22f.jpg
    width: 1200
    height: 631
  color: '#010509'
---

The HydraFusion research preview is now available in Visual Studio Code and the GitHub Copilot app, expanding beyond Copilot CLI.

HydraFusion appears in the model picker, but rather than being a single model, it orchestrates multiple models. HydraFusion treats workflow selection as an optimization problem. It uses capability signals for reasoning, code generation, debugging, and tool use to select the most efficient execution pattern to meet the quality bar. HydraFusion uses one of three workflows:

- **Single:** One selected model directly solves the task.
- **Cascade:** An efficient model drafts a solution and a quality gate decides whether to accept it or escalate to a stronger model.
- **Critique:** One model drafts a result, an independent read-only critic from a different model family reviews it—following the same review pattern as [Rubber Duck](https://docs.github.com/copilot/concepts/agents/copilot-cli/rubber-duck)—and the drafting model revises once.

### [Get started](https://github.blog/changelog/2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app#get-started)

In VS Code (version 1.140 or later, or VS Code Insiders):

1. Select **HydraFusion** from the Copilot Chat model picker.
2. If it does not appear, enable `chat.copilot.hydraFusion.enabled`. If you get Copilot through an organization or enterprise, an administrator may need to allow preview features in organization or enterprise settings.

In the GitHub Copilot app:

1. Update to the latest version.
2. Open **Settings**, search for “HydraFusion”, and turn it on.
3. Select **HydraFusion** from the model picker.

### [What’s improved](https://github.blog/changelog/2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app#whats-improved)

Bringing HydraFusion to more surfaces was the top request from early feedback. This release also improves the overall experience:

- **Greater transparency:** See more clearly what HydraFusion is doing at each step of a workflow.
- **Real-time progress:** HydraFusion now sends more frequent progress updates while it works.
- **Clearer status during long tasks:** Progress updates make it easier to tell that HydraFusion is still working, even when a task takes longer.

### [How HydraFusion differs from Auto](https://github.blog/changelog/2026-09-30-hydrafusion-in-vs-code-and-the-github-copilot-app#how-hydrafusion-differs-from-auto)

Auto selects a model for each request. HydraFusion explores how Copilot can both select a workflow and coordinate multiple models within a turn.

HydraFusion is available to Copilot Pro, Pro+, Business, and Enterprise users. For Copilot Business and Enterprise, an administrator must enable preview features. HydraFusion remains a research preview and is subject to change.

Learn more in the [HydraFusion documentation](https://docs.github.com/early-access/copilot/hydrafusion), read about the research and benchmark results in [Project HydraFusion: Frontier quality via multi-model orchestration](https://github.blog/ai-and-ml/github-copilot/project-hydrafusion-frontier-quality-via-multi-model-orchestration/), or share feedback in the [GitHub Community](https://github.com/orgs/community/discussions/206492).
