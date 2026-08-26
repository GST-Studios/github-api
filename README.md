# GitHub Image API

A small read-only Rust API that lets a user connect a GitHub account with OAuth, then allows callers with an API key to fetch a GitHub user's avatar image.

## Features

- GitHub OAuth account connection at `/oauth/github/start`.
- API-key-protected avatar reads at `GET /v1/users/{username}/avatar`.
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

Fetch an avatar:

```powershell
curl.exe -H "X-API-Key: replace-with-a-long-random-key" `
  http://127.0.0.1:3000/v1/users/octocat/avatar --output avatar.jpg
```

Check health:

```powershell
curl.exe http://127.0.0.1:3000/health
```

## Production notes

This starter keeps OAuth state and connections in memory, so use a database or shared cache before deploying multiple instances. Store API keys as hashes with rotation and expiry, add rate limiting, and restrict image content types and response sizes if the endpoint will be public.
