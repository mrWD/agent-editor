# Setting the editor up: a guide for the agent

You are an agent inside Agent Editor, and the user asked you to set the editor
up for their work. The editor gives you tools for this on its own tool server
(`agent-editor`). This guide says what "working" means and how to get there.
Follow it in order and stop to ask the user only where it says so.

## What the editor needs

| Part | What makes it work | Who can do it |
|---|---|---|
| Projects | A git repository on this computer, added as a project. | You: `add_project`. |
| GitHub | `gh` installed and signed in to the project's server. | The user signs in; you check. |
| GitLab, also a company's own | `glab` installed and signed in to the project's server. | The user signs in; you check. |
| Linear | A personal API key, and per project the team its issues come from. | The user pastes the key; you pick the team: `set_linear_team`. |
| Docker | Installed and running, only for projects with a compose file. | The user. |
| Agents | Node.js with `npx`. Claude, Codex and Gemini start through `npx` and sign in on first use. | The user signs in once per agent. |

The editor never sees a GitHub or GitLab token: it asks `gh` and `glab`,
which keep their own sign-in. Never ask the user for one of those tokens and
never put one in a file or a command line.

## Step by step

1. **Look first.** Call `setup_status`. It lists the projects with the server
   each one lives on and where its issues come from, whether `gh`, `glab` and
   Docker are ready, and whether Linear is connected. Read it before asking the
   user anything: most answers are already there.

2. **Ask what the team uses**, only what `setup_status` cannot tell: which
   tracker holds the issues (GitHub, GitLab or Linear), and for Linear which
   team each project belongs to. If the user already said it in their message,
   do not ask again.

3. **Projects.** If a repository the user works in is not a project yet, add it
   with `add_project` and the folder's full path. The folder must be a git
   repository. The project's server is read from its `origin` remote.

4. **The code host.**
   - A project on GitHub needs `gh`; on GitLab, `glab`. Check with
     `gh --version` or `glab --version`. If it is missing, say how to install
     it (on a Mac: `brew install gh` or `brew install glab`) and let the user
     run it.
   - Signing in is the user's step, because it opens a browser or asks for a
     token. For a company's own GitLab, give the exact command with the
     server from `setup_status`, for example
     `glab auth login --hostname gitlab.example.com`. Tell the user they can
     run it in the editor's Terminal tab. Then call `setup_status` again to
     see that it worked.
   - The editor knows a GitLab server by "gitlab" in its address
     (`gitlab.example.com`, `git.gitlab.example.com`). A GitLab at an address
     without that word is not recognised: tell the user, and set its issues to
     come from Linear if the team uses Linear.

5. **Linear.**
   - Without a key, ask the user to create a personal API key at
     <https://linear.app/settings/account/security> and paste it in the
     editor's **Issues** screen, in the Linear box. Do not ask them to paste
     it into the conversation. Another way is to start the core with
     `LINEAR_API_KEY` set in its environment.
   - With a key, `setup_status` lists the workspace's teams. Set each
     project's team with `set_linear_team`, using the team key such as `APP`.
     Several projects may share one team. From then on, the project's issues
     come from that team, starting a task moves its issue to In progress and
     merging moves it to Done. Merge requests still go through the code host.
   - Leave a project on its code host (no team) if its issues live there.

6. **Check it works.** Call `setup_status` once more. Everything the user
   uses should read as ready. Then tell the user in a few lines what is set
   up, what is left for them to do (with the exact command or screen), and
   that the Issues screen now lists their issues.

## What not to do

- Do not change code, commit, push or open merge requests while setting up.
- Do not install or sign in to anything without the user's yes.
- Do not read keys or tokens out of files, the keychain or other programs'
  settings, and do not write them anywhere.
- Do not change a project's team without knowing it is the right one: ask.
