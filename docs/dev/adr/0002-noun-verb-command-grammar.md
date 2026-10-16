# 0002. Noun-verb command grammar

Status: Accepted
Date: 2026-10-16

## Context
The first feature set is dataset management. Training, models and robots follow. Flat verbs (`imify upload`) stop scaling at the second resource.

## Decision
Commands are `imify <noun> <verb>`: `imify datasets ls`, `imify datasets upload`. Account-level commands are top-level verbs: `imify login`, `imify logout`, `imify whoami`, `imify doctor`. Nouns are plural.

## Consequences
New resources add a noun without touching existing commands. The API's resource names, not CLI-specific concepts, define the nouns.
