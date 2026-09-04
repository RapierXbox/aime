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


### Auth
the auth conists of two main parts, both of which are pq.
#### master backup key
this is a on device generated 128bit key used for encrypting your backups. this is never send in any way to the server.
you can transfer it to other devices via a qr code. 24 words or via your icloud keychain. if possible its encryptet in storage via biometrics.
#### devices sessions and enrollment
do enroll a new device you can create a enrollment token. its single use and has a short ttl. it can be generated via account password, a qr from another device or a mail link. then you generate a public and private keypair on your device. you then enroll your device using the public key and a enrollment token. 
when you then want to pull a backup or use a encoding endpoint you can use your public key with your device id to generate a session token by generating a challange and solving it with your sk. its also possible to disable some login methods for more security. 