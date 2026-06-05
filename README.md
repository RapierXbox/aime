# AIME
Yet another AI SaaS project. Made for the Check24 CodeClub Mentorship.

AIME is the best and most secure way to add a knowlageable to your existing job / company. It interfaces directly with your mail (and more maybe). All your data gets stored on a client local rag database. The server never stores anything, except a fully encrypted backup for multi-device use if you opt in. For now, the server does the compute but has a strict no logging policy (some usage data will be tracked for billing but no sensible data will be reconstructable).

### Goals:
- A all knowing agent you can use for your projects
- Automatic reply suggestions using
- Prompt Injection Scanning and PII
- FHE Embedding for full data safety (later)
- Completly client stored data approach

### Stack:
- Backend:
    - Golang with cgo + Postgres
- Frontend:
    - Tauri + React SPA (vite, tailwind, shadcn/ui)
- Deployed using Ansible and Docker Compose

### Getting Started:
Install https://taskfile.dev
run *task* to see all commands
run *task setup* to setup everything