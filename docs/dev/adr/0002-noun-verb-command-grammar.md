# 0002. Noun-verb command grammar

Status: Accepted
Date: 2026-10-16

## Context
The first feature set is dataset management. Training, models and robots follow. Flat verbs (`imify upload`) stop scaling at the second resource.

## Decision
Commands are `imify <noun> <verb>`: `imify datasets list`, `imify datasets upload`. Nouns are plural. Account commands live under `auth`: `imify auth login`, `imify auth logout`, `imify auth whoami`. `auth` and `config` are singular, because they are not API resources. `imify doctor` stays a top-level verb.

## Consequences
New resources add a noun without touching existing commands. The API's resource names, not CLI-specific concepts, define the nouns.
