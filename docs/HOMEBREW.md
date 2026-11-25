# Публикация через один репозиторий

Используем только **merklol/rmpkgx**: исходники и `Formula/rmpkgx.rb` находятся
в одном репозитории. Отдельный `homebrew-tap` не нужен. Скрипты ничего не публикуют.

## 1. Подготовка и загрузка кода

```sh
python3 scripts/prepare-homebrew.py --owner merklol
git add .
git commit -m "Prepare single-repository Homebrew installation"
git push -u origin main
```

Git-репозиторий и origin уже должны существовать. Если push просит пароль GitHub,
используй personal access token с Contents и Workflows Read and write вместо
пароля аккаунта либо настрой SSH. Сам токен не вставляй в команды и не публикуй.

В Git загружается `Formula/rmpkgx.rb`, исходники и документация. `dist/` и `target/`
игнорируются. Старую сгенерированную папку `dist/homebrew/homebrew-tap/` не используй.

## 2. Опубликовать релиз

Открой https://github.com/merklol/rmpkgx/releases/new и создай релиз **v0.1.0**
для загруженного кода. Прикрепи ровно эти два файла:

- `dist/homebrew/rmpkgx-v0.1.0-source.tar.gz`
- `dist/homebrew/rmpkgx-v0.1.0-source.tar.gz.sha256`

Папку можно открыть командой `open dist/homebrew`.

Используй только архив от последнего запуска генератора: его SHA-256 уже записан
в корневую formula. Не подставляй автоматический GitHub Source code (tar.gz)
или архив готового arm64-бинарника. Если release v0.1.0 уже опубликован и доступен
пользователям, не заменяй его другими байтами: сначала увеличь версию и выпусти новый тег.

Архив содержит исходники, Cargo.lock, тесты и документацию по явному списку
каталогов. `Formula/` исключена, чтобы запись SHA-256 не меняла сам архив;
`target/`, `dist/`, `.git`, корневая `.env` и Python-кеш также исключены.
Ссылки отклоняются, даты и владельцы нормализуются. Перед выпуском проверь,
что секреты не добавлены в сами исходники или документацию.

## 3. Проверить Homebrew

После публикации source-архива:

```sh
brew tap merklol/rmpkgx https://github.com/merklol/rmpkgx.git
brew trust --formula merklol/rmpkgx/rmpkgx
brew install --build-from-source merklol/rmpkgx/rmpkgx
brew test merklol/rmpkgx/rmpkgx
rmpkgx --version
rmpkgx
```

Явный URL позволяет использовать репозиторий `rmpkgx` без префикса `homebrew-`.
В новых версиях Homebrew необходимо доверие конкретной formula. Если команда
`brew trust` неизвестна, сначала обнови Homebrew.

Вкладка Actions → **Homebrew source installation** → Run workflow запускает
настоящие `brew install` и `brew test` на Apple Silicon. Workflow ручной, чтобы
не пытаться установить ещё не опубликованный release asset. Он ничего не публикует.

Developer ID и нотаризация для этой source formula не требуются. Rust — зависимость
сборки, Cargo-зависимости скачиваются на стадии fetch; install и сама утилита работают
без сети. Bottles не подготовлены: сборка идёт на компьютере пользователя.

Локальная среда проекта запрещает запись в `/opt/homebrew`, поэтому изолированный
Cargo-install и загрузка Homebrew DSL не заменяют реальный brew install. Финальную
проверку установки и workflow нужно выполнить после публикации. `brew test` не
удаляет приложения и не сканирует пользовательскую Library.

## Обновления

Измени версию Cargo.toml и CHANGELOG.md, затем повтори генерацию, загрузку кода
и публикацию source assets. Пользователь обновляет так:

```sh
brew update
brew upgrade merklol/rmpkgx/rmpkgx
```

Удаление: `brew uninstall merklol/rmpkgx/rmpkgx`. Журналы rmpkgx сохраняются.

[Homebrew tap documentation](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap)
