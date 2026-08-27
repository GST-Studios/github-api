# GitHub Image API

A small read-only Rust API that lets a user connect a GitHub account with OAuth, then allows callers with an API key to fetch GitHub user data or an avatar image.

## Features

- GitHub OAuth account connection at `/oauth/github/start`.
- API-key-protected complete public profile reads at `GET /v1/users/{username}`.
- API-key-protected avatar reads at `GET /v1/users/{username}/avatar`.
- API-key-protected repository reads at `GET /v1/users/{username}/repos`, including private repositories the connected account can access.
- Repository name/link reads at `GET /v1/users/{username}/repositories`.
- Recursive file/folder name/link reads at `GET /v1/users/{username}/repos/{repo}/tree`.
- Organization repository reads at `GET /v1/orgs/{organization}/repositories`.
- Organization file/folder reads at `GET /v1/orgs/{organization}/repos/{repo}/tree`.
- No write routes to GitHub and no editing permissions requested.
- In-memory OAuth state and linked-account storage for the starter project.

## Run locally

1. Create a GitHub OAuth App under **Settings > Developer settings > OAuth Apps**.
2. Set its callback URL to the value in `GITHUB_REDIRECT_URI`.
3. Copy `.env.example` to `.env` and fill in the values.
4. Run:

```powershell
cargo run
```

Connect GitHub by opening `http://127.0.0.1:3000/oauth/github/start`.

The OAuth flow requests `read:user` and `repo` permission. Reconnect an account after upgrading this API so GitHub can grant the new private-repository permission.

Fetch an avatar:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/users/octocat/avatar --output avatar.jpg
```

Fetch the complete public GitHub profile as JSON:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/users/octocat
```

The profile endpoint returns GitHub's read-only public user fields, including the avatar URL, bio, company, location, follower counts, repository counts, and profile URLs. It does not modify GitHub data.

Fetch repositories for a connected account, including private repositories:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/users/MICKYcyber/repos
```

The repository endpoint uses the connected account's OAuth token and returns only data from GitHub's read API. The API never creates, updates, deletes, or pushes to repositories.

For an organization such as `GST-Studios`, the connected GitHub account must be a member of that organization. Use the organization routes so the API can verify membership and read repositories visible to that account:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/orgs/GST-Studios/repositories
```

The organization route returns `403` until a connected OAuth account is confirmed as an organization member.

List only repository names and links:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/users/MICKYcyber/repositories
```

List every file and folder name and its GitHub link. Add `?branch=main` to choose a branch; otherwise GitHub's default branch is used:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  "http://127.0.0.1:3000/v1/users/MICKYcyber/repos/your-repository/tree?branch=main"
```

Check health:

```powershell
curl.exe http://127.0.0.1:3000/health
```

## Production notes

OAuth tokens are saved in `.oauth_tokens.json` so the connection survives API restarts. This file is ignored by Git and should be protected like a credential. Set `OAUTH_STORE_PATH` to move it elsewhere. Reconnect only if the token is revoked or its permissions change.

The classic GitHub OAuth `repo` scope grants broad repository permissions to the token; the API itself only calls read endpoints. For least privilege in production, use a GitHub App with repository contents and metadata set to read-only. Store API keys as hashes with rotation and expiry, add rate limiting, and restrict image content types and response sizes if the endpoint will be public.
