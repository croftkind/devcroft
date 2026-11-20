# Devcrof V2 Ideas

I have been dogfooding devcroft for months. During this testing I have discovered limitation of our current approach. Also, I have exposed to few more ideas that feels more natural for agent driven development. So, I am planning re-architeching devcroft. It will be singificantly different than what we have currently. This doc discuss what are the limitation of current arhitecture and what I expect from v2. V2 does not means v2 version of devcroft. It just means v2 version of our approach for this project.

The UI will be dastrically different than what we have currently. There should be easy way to discover contextual information. More focus on what matter most and less to what used rarely. It still should support rich keyboard navigation. There should be one preffered way to do a thing rather than giving options. For example, only built-in editor. No neovim. User can still open a project in their preffered IDE.

## Limitation of current architecture

### Rich interaction not possible in TUI

TUI has become a lot nicer these days. Lot's of rich interaction is possible now. However, it still lack compares to GUI. Some rich interaction is not possible in TUI. For example, image preview. Flowchart/ mermaid diagram rendering. Threaded conversation. Rich file reference/link handling. With a native GUI, agent can render conversation more nicely.

### Different interaction model for differnt harness

Currently, to use different agents from different provider I have to use their own harness cli. They have different interaction model. Same thing works differently in different harness. They have their own set of commands and features. We need an unified interaction model so that user don't have to learn how to use different tools.

### Repository scoped workspace

Currently, the workflow evolves around a repository. However, real world work require working on a multiple repositories for same feature. Currently, it is ambgious where the seesion for a task goes when it touches multiple repositories. Also, it is not convinient to review changes when single sesson changes multiple repositories.

### No good task management model

We currently don't have a good task model. As task may require change in multiple repository and may produce PR across related repositoreies. There is no good options to show task progress or state. There is no way to discover tasks. Task should be treated as first class citizen. Not an after effect.

### Artifact discovery is not good enough

Current artifact discovery model is ambigious. An artifact can involve multiple repositories. Also, we need a way to discover artifacts for a particular task.

### UI is not as intutive as I expected

We made some radical choice for the UI. It differ from any other ai assisted development tools. However, some features are not very useful as I expected would be. The keyboard driven navigaton is nice and I love it. However, some other features are not that useful or intutive.  For example, the home page really does not provide any value. I have rarely used any feature of it. The session sidebar is not as useful as I expected to be. Ongoing agent session indicator is not that intuative. Some feature are only discoverable in command pallete. We need a better way to let user discover the features.


## Core Concepts for Devcroft V2

### Profile

Profile provides isolation for the agents. I should be able to use different subscription for same provider in different profile. For example, I use claude for both work and my personal task. I should be able to use my company subscription on work profile and my personal subscription on my personal profile. Some requirements for profiles:
- Differnet profile can use different subscription for same provider
- Profile may have some profile specific skills
- Maybe profile specific instruction for agent
- We should have a way to track usage of different subscriptions under a profile

### Workspace

A group of related repository. Workspace is the center piece of development workflow. If working on a task changes multiple repository, they should go into same workspace. An workspace is a dedicated folder on the machine. All related repositories will be cloned there. If a repository belongs to multiple workspaces it will be cloned in multiple places. A workspace should have everything we need to work on a task. Some requirements for workspace:
- An index of the repository and their relationship. This will be generated with agent. It might be refreshed periodically. Agent should be able to use that index in most of cases instead of exploring the repository every time to decide whether the repository relates to a task or not.
- All agent sessions should start on workspace root folder.
- Workspace level AGENTS.md
- Workspace level skills
- A workspace belongs to exactly one profile
- Workspace should inherit skills and instruction from their profile
- Workspace level artifact discovery
- Workspace level task discovery and management
- Workspace level pull request discovery and review facility

### Task

A deliverible work. It is workspace scoped. It can involves multiple repositories thus may produce multiple pull requests. Some requirements for task:

- It belongs to exactly one workspace
- It must not change any repository outside of workspace
- It can change multiple repository and may produce multiple pull requests
- A task may produce/consume multiple workspace scoped artifacts
- There should be way to discover all artifacts for a task
- There shold be a way to discover all pull requests for a task
- Task has state like todo, in-progress, done etc.
- There should be a parent agent session and might be multiple threads/sub-sessions
- There should be easy way to view the changes made by a task across multiple repositories
- There might be multiple tasks in-progress parallely they might change same repositories. So, maybe git worktree makes sense here.

### Artifacts
- Rich and beutiful artifacts like claude artifacts
- It doesn't have to be markdown like currently we have
- Rich HTML artifcats with interactive visual should be supported
- Agent may present some UI prototype as in the artifact we must support this
- Artifacts are workspace scoped.
- There should be way to discover artifacts within workspace
- A task can produce multiple artifacts
- There should be a way to discover related artifacts within task
- There should be way to ask agent about a selection or provide feedback for an artifact

### Terminal Sessions

Terminal session is similar as our current libghostty powered terminal session but they now belongs to a workspace and/or task.
- Terminal session belongs to a repository. A session may start at repository root or workspace root.
- Each repository should have their own terminal sessions/tabs.
- Terminal session should be task aware. So, if a task is being done on a git work-tree for a repository, terminal should be automatically switched to that.
- There should be a easy way to navigate between terminal session for different repositories for a workspace

### Agent Sessions

An unified frontend for different agent across different providers.
- An unified rich graphical chat UI like T3Code
- Should support multiple provider without switching interfaces
- We should be able to select provider, model, and effort level
- An agent session can be scoped to a task or can be workspace scoped
- We should support threaded session. So, there could be parent session for a task and then there could be multiple threads from that parent session
- There should be a easy way to discover the threads.
- Chat should support rich visual like table, mermaid etc.
- Should support referencing files, artifacts within the workspace
- Should support question, permision promt etc.

### Pull Request Review

Pull request review should be first class citizen. We should review PR from github.
- Pull requests are workspace scoped
- There should be a way to discover PRs for the repositories of a particular workspace
- There should be a way to mark a PR as watched. Then, there should be notification for event like PR approved, merged etc.
- We should be able to filter PR by created by, participating, state etc.
- There should be interactive agent thread for a PR review.
- There should be a way to select a codeblocks and ask question about it to agent.
- Should be able to approve a PR that get synced in Github
- Github comments should be visible
- Should be able to post comment to github from review ui
- Should see github checks status

### General Chats

General chats are not scoped to a particular workspace. There should be a dedicated place for chatting with agent for ideas, machine isuees or general questions.
- Not workspace scoped
- A dedicated UI for general chats
- Maybe thread like workspace chat
- It can start in a dedicated directory instead of machine root
- It can be scoped to profile

### Tools

General tools that helps everyday task. For example, JSON formatter, file diff viwer, base64 encoder/decoder etc. We already have these currently but they are not easy to discover. There should be a dediced UI to discover the tools.

### Wiki

A wiki or knowledgebase where agent can read from or write to. Not only agent but human can add update content there. It will be home for knowldge like style guide, design patterns, architectural guideline, trips and ticks, list of useful libraries or tools etc. User can refer an wiki resource to an agent or agent can automatically search in the wiki to see if there anything related to it's current task. A wiki is global knowledge base. It is not scoped to any workspace or profile.

### Inbox

A notification system for informing user for things like some agent finished execution, some agent need attention, A PR I am watching have been merged etc.

### Remote workspace

In future we may support remote workspace. Maybe connect over ssh to other the machine on network. Maybe it can be a cloud machine. The details are not clear yet. It is a future possibility not current requirement. So, we should design our workspace concept so that we can later introduce remote workspace later.

### Must support providers

We must support following providers/harness:
- OpenAI Codex
- Antropic claude
- Google Antigravity
- Opencode
