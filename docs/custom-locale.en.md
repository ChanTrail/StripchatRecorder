# Custom Locale

[简体中文](custom-locale.md) | [English](custom-locale.en.md)

This document explains how to customize UI translations or add new languages in StripchatRecorder.

Translations are loaded from **JSON files on disk** rather than being compiled into the binary. The program creates default locale files on first run, which you can edit freely — your changes are never overwritten by updates.

---

## Directory Structure

```
<exe_dir>/
└── locale/
    ├── app/                        # Main application translations
    │   ├── zh-CN.json              # Simplified Chinese (default)
    │   └── en-US.json              # English
    ├── log/                        # Backend log translations (affects log output language)
    │   ├── zh-CN.json
    │   └── en-US.json
    └── modules/                    # Module translations
        ├── filter_short/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── contact_sheet/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── notify_discord/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── notify_telegram/
        │   ├── zh-CN.json
        │   └── en-US.json
        ├── cleanup/
        │   ├── zh-CN.json
        │   └── en-US.json
        └── __builtin__/            # Built-in DAG node translations (recording_input, unpack)
            ├── zh-CN.json
            └── en-US.json
```

> **Note:** Files are created automatically on first run. If a file already exists it is **never overwritten**, so your customizations are preserved across restarts and updates.
>
> Exception: if a built-in locale file (`app/`, `log/`, `modules/<builtin_id>/`) exists but fails JSON validation, the program automatically rebuilds it with default content. Custom locale files (third-party modules or custom locale codes) only log a warning on validation failure and are not rebuilt.

---

## Modifying Existing Translations

Simply edit the JSON file for the desired language under `locale/app/` or `locale/modules/<module_id>/`.

The frontend fetches `/api/locale/{code}` on startup and on every language switch, so your changes take effect immediately after **reloading the page** — no rebuild required.

---

## Adding a New Language

### 1. Create the locale file

Create a new file under `locale/app/` using a BCP 47 language tag as the filename, e.g. `ja-JP.json`.

Copy `en-US.json` as a template and translate all string values (keep all keys and `{variable}` placeholders unchanged). Set the `languageName` field to the language's own native name:

```json
{
  "languageName": "日本語",
  "nav": {
    "streamers": "配信者",
    ...
  },
  ...
}
```

**That's it — no code changes required.** After restarting the program, `/api/locales` automatically includes the new language and both the Settings page and the setup wizard will display it in the language list.

---

## Module Translations

Each module has its own folder under `locale/modules/<module_id>/`. Place a `{locale_code}.json` file there with the following structure:

```json
{
  "name": "My Module",
  "description": "Module description",
  "params": {
    "param_key": { "label": "Param label" }
  }
}
```

All fields are optional — any omitted field falls back to the module's own `--describe` JSON value.

> **Priority:** Server-side locale JSON overrides `--describe` i18n → `--describe` i18n overrides the original default value.

For third-party modules (not in the built-in list), simply create a folder with the module's `id` under `locale/modules/` and add the JSON files — the system discovers them automatically.

### Built-in DAG node translations (`__builtin__`)

The `__builtin__` module locale file uses a **per-node nested structure**, unlike the flat structure of regular modules:

```json
{
  "recording_input": {
    "name": "Recording Input",
    "description": "Virtual recording input node — always the pipeline start point"
  },
  "unpack": {
    "name": "Unpack Media Bundle",
    "description": "Split a media bundle into a video file (port 0) and an image file (port 1)"
  }
}
```

---

## Translation Key Reference

| Top-level key    | Description                                                   |
| ---------------- | ------------------------------------------------------------- |
| `nav`            | Sidebar navigation labels                                     |
| `common`         | Shared button text (confirm, cancel…)                        |
| `notify`         | System notifications and dialogs (disconnect, missing files…) |
| `home`           | Streamers list page                                           |
| `streamerCard`   | Streamer card component                                       |
| `addStreamer`    | Add streamer dialog                                           |
| `recordings`     | Recordings page                                               |
| `postprocess`    | Post-processing pipeline page                                 |
| `relay`          | Relay streams page                                            |
| `finder`         | Streamer finder page                                          |
| `settings`       | Settings page                                                 |
| `usePostprocess` | Post-processing task status messages                          |
| `setup`          | First-launch setup wizard                                     |
| `login`          | Login page (password input, initial password setup)           |
| `notifications`  | In-app notification panel                                     |
| `community`      | Community module marketplace page                             |
| `about`          | About page (version info, update check)                       |
| `streamers`      | Streamer event messages (rename notifications, API errors…)  |
| `dirBrowser`     | Directory picker dialog                                       |

Interpolation variables use the `{variableName}` format. Always keep placeholders as-is when translating:

```json
"reconnected": "Reconnected to server, reloading in {n} second(s)…"
```
