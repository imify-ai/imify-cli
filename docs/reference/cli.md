# CLI reference

## imify

```text
Command-line interface for the imify platform

Usage: imify [OPTIONS] [COMMAND]

Commands:
  doctor    Check the configuration, the API and the access token
  auth      Sign in, sign out and show the signed-in user
  config    Read and change the settings in the config file
  datasets  Work with datasets
  policies  Work with policies: the models trained on a dataset version
  self      Manage this imify installation
  help      Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

  -V, --version
          Print version
```

## imify doctor

```text
Check the configuration, the API and the access token.

Writes a report to stdout. Exits with the error code of the first failing check, or 0 when no check fails. Contacts only the API host.

Usage: imify doctor [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Examples:
  imify doctor --api-url https://api.example.com
  imify doctor --profile staging
```

## imify auth

```text
Sign in, sign out and show the signed-in user

Usage: imify auth [OPTIONS] <COMMAND>

Commands:
  login   Sign in through the browser and store the login for the host
  logout  Revoke and remove the stored login for the host
  whoami  Show the user the access token belongs to
  help    Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Examples:
  imify auth login
  imify auth whoami
  imify auth logout
```

## imify auth login

```text
Sign in through the browser and store the login for the API host.

Opens the browser and waits up to 5 minutes. The login is stored in the OS keychain, or in a file readable only by the user when no keychain is available.

Usage: imify auth login [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

      --no-browser
          Print the sign-in URL and read the redirect URL from the terminal instead of opening a browser

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Examples:
  imify auth login
  imify auth login --api-url https://api.example.com
  imify auth login --no-browser
```

## imify auth logout

```text
Revoke and remove the stored login for the host

Usage: imify auth logout [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify auth logout
```

## imify auth whoami

```text
Show the user the access token belongs to

Usage: imify auth whoami [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify auth whoami
```

## imify config

```text
Read and change the settings in the config file

Usage: imify config [OPTIONS] <COMMAND>

Commands:
  get       Print a setting's value and where it comes from: flag, env, config or default
  set       Check a value and store it in the config file
  unset     Remove a setting from the config file
  list      Print every setting with its value and where it comes from
  profiles  List the profiles in the config file with the API URL of each
  help      Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help
```

## imify config get

```text
Print a setting's value and where it comes from: flag, env, config or default

Usage: imify config get [OPTIONS] <KEY>

Arguments:
  <KEY>
          The setting

          Possible values:
          - api-url: The API host every networked command talks to

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Example:
  imify config get api-url
```

## imify config set

```text
Check a value and store it in the config file

Usage: imify config set [OPTIONS] <KEY> <VALUE>

Arguments:
  <KEY>
          The setting

          Possible values:
          - api-url: The API host every networked command talks to

  <VALUE>
          The new value

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Example:
  imify config set api-url https://api.example.com
```

## imify config unset

```text
Remove a setting from the config file

Usage: imify config unset [OPTIONS] <KEY>

Arguments:
  <KEY>
          The setting

          Possible values:
          - api-url: The API host every networked command talks to

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Example:
  imify config unset api-url
```

## imify config list

```text
Print every setting with its value and where it comes from

Usage: imify config list [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify config list --json
```

## imify config profiles

```text
List the profiles in the config file with the API URL of each

Usage: imify config profiles [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify config profiles
```

## imify datasets

```text
Work with datasets

Usage: imify datasets [OPTIONS] <COMMAND>

Commands:
  ls        List the datasets visible to the access token's user
  validate  Check a local LeRobot dataset before an upload
  upload    Upload a local LeRobot dataset into an ingest session
  help      Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help
```

## imify datasets ls

```text
List the datasets visible to the access token's user

Usage: imify datasets ls [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

      --task <TEXT>
          Keep datasets whose task contains TEXT, ignoring case

      --format <FORMAT>
          Keep datasets with exactly this format, such as `lerobot_v3`

  -q, --quiet
          Write the result only

      --embodiment <EMBODIMENT>
          Keep datasets with exactly this embodiment label, such as `so100`

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --sort <FIELD>
          Sort by FIELD, ties by id. Without it: by task, then embodiment, then id
          
          [possible values: id, task, format, embodiment, episodes, accepted]

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --reverse
          Reverse the order

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Examples:
  imify datasets ls
  imify datasets ls --embodiment so100 --sort episodes --reverse
  imify datasets ls --task cube --json
```

## imify datasets validate

```text
Check a local LeRobot dataset before an upload.

Writes the report to stdout and exits with 5 when it has an error. Needs no network access and no token.

Usage: imify datasets validate [OPTIONS] <PATH>

Arguments:
  <PATH>
          The dataset root: the directory that contains meta/info.json

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Example:
  imify datasets validate ./pick-cube
```

## imify datasets upload

```text
Upload a local LeRobot dataset into an ingest session.

Validates the dataset first and uploads nothing when the report has an error. Uploads meta/, data/ and videos/, with meta/info.json first. Writes the ingest session to stdout. Ctrl-C stops the upload and leaves the session open. Running the same upload again continues that session and sends only the files it lacks.

Usage: imify datasets upload [OPTIONS] --task <TEXT> <PATH>

Arguments:
  <PATH>
          The dataset root: the directory that contains meta/info.json

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

      --task <TEXT>
          Task description of the recorded episodes

      --dry-run
          Validate and list the files to upload; needs no token and no network

  -q, --quiet
          Write the result only

      --resume
          Continue the interrupted upload of PATH; fail when there is none

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --restart
          Discard the interrupted upload of PATH and start a new ingest session

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Examples:
  imify datasets upload ./pick-cube --task "Pick up the red cube"
  imify datasets upload ./pick-cube --task "Pick up the red cube" --dry-run
  imify datasets upload ./pick-cube --task "Pick up the red cube" --restart
```

## imify policies

```text
Work with policies: the models trained on a dataset version

Usage: imify policies [OPTIONS] <COMMAND>

Commands:
  list  List the policies visible to the access token's user
  show  Show a policy, what it was trained on and its checkpoints
  help  Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help
```

## imify policies list

```text
List the policies visible to the access token's user

Usage: imify policies list [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

      --state <STATE>
          Keep policies in this state
          
          [possible values: queued, training, ready, failed]

      --dataset <ID>
          Keep policies trained on the dataset with this id

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Examples:
  imify policies list
  imify policies list --state ready --json
  imify policies list --dataset 0b6f3c2e-8d1a-4f7e-9a51-2c4d6e8f0a13
```

## imify policies show

```text
Show a policy, what it was trained on and its checkpoints

Usage: imify policies show [OPTIONS] <POLICY>

Arguments:
  <POLICY>
          The policy id

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify policies show run-3c9e1a7f
```

## imify self

```text
Manage this imify installation

Usage: imify self [OPTIONS] <COMMAND>

Commands:
  update  Update imify to the latest release
  help    Print this message or the help of the given subcommand(s)

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help

Example:
  imify self update
```

## imify self update

```text
Update imify to the latest release.

Works for an install by the imify installer: looks up the latest release on GitHub and, when it is newer, runs its installer for the install directory recorded in the install receipt. Other installs fail with a hint naming their update command.

Usage: imify self update [OPTIONS]

Options:
      --json
          Write JSON instead of text [env: IMIFY_OUTPUT=json]

  -q, --quiet
          Write the result only

  -v, --verbose...
          Log more detail to stderr: -v for debug, -vv for trace

      --yes
          Confirm without prompting

      --no-color
          Disable color [env: NO_COLOR]

      --api-url <URL>
          API host [env: IMIFY_API_URL] [default: profile, then https://platform.imify.ai]

      --profile <NAME>
          Settings profile in the config file [env: IMIFY_PROFILE] [default: default]

  -h, --help
          Print help (see a summary with '-h')

Example:
  imify self update
```
