# Venera Edge
[![flutter](https://img.shields.io/badge/flutter-3.41.4-blue)](https://flutter.dev/)
[![License](https://img.shields.io/github/license/Yo1sing/Venera-Edge)](https://github.com/Yo1sing/Venera-Edge/blob/master/LICENSE)
[![stars](https://img.shields.io/github/stars/Yo1sing/Venera-Edge?style=flat)](https://github.com/Yo1sing/Venera-Edge/stargazers)

[![Download](https://img.shields.io/github/v/release/Yo1sing/Venera-Edge)](https://github.com/Yo1sing/Venera-Edge/releases)

A comic reader that supports reading local and network comics, forked from [Venera](https://github.com/venera-app/venera).

Application ID: `io.github.yo1sing.venera.edge`.

The AltStore source is initialized without upstream releases. After publishing the first Venera Edge IPA, run `update_alt_store.py` to populate it with this fork's release information.

## Features
- Read local comics
- Use javascript to create comic sources
- Read comics from network sources
- Manage favorite comics
- Download comics
- View comments, tags, and other information of comics if the source supports
- Login to comment, rate, and other operations if the source supports

## Build from source
1. Clone the repository
2. Install flutter, see [flutter.dev](https://flutter.dev/docs/get-started/install)
3. Install rust, see [rustup.rs](https://rustup.rs/)
4. Build for your platform: e.g. `flutter build apk`

## Create a new comic source
See [Comic Source](doc/comic_source.md)

## Thanks

### Tags Translation
[EhTagTranslation](https://github.com/EhTagTranslation/Database)

The Chinese translation of the manga tags is from this project.

## Headless Mode
See [Headless Doc](doc/headless_doc.md)

