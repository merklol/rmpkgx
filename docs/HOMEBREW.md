# Публикация rmpkgx в собственном Homebrew tap

Все загрузки выполняешь ты. Скрипты не создают репозитории, не делают коммиты,
не отправляют теги и не публикуют релизы. Это независимый tap, а не Homebrew Core.
Developer ID, платная программа Apple и нотаризация для source formula не нужны.

Нужны два публичных репозитория одного владельца:

- `YOUR_LOGIN/rmpkgx` — код и релизные файлы;
- `YOUR_LOGIN/homebrew-tap` — содержимое сгенерированной папки tap.

`YOUR_LOGIN` ниже замени на GitHub-логин или организацию. Имена репозиториев
фиксированы генератором; `homebrew-tap` устанавливается как `YOUR_LOGIN/tap`.

## 1. Подготовить исходники

На Apple Silicon Mac со stable Rust и Python 3.11+:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
python3 scripts/test-prepare-homebrew.py
python3 scripts/prepare-homebrew.py --owner YOUR_LOGIN
```

Результат в `dist/homebrew/`:

```text
rmpkgx-v0.1.0-source.tar.gz
rmpkgx-v0.1.0-source.tar.gz.sha256
homebrew-tap/
  Formula/rmpkgx.rb
  README.md
  LICENSE
  .github/workflows/test.yml
```

Архив содержит Cargo.toml, Cargo.lock, исходники, тесты и документацию.
Сборочные файлы `target/`, артефакты `dist/`, `.git` и корневая `.env` исключены.
Файлы исходников включаются по явному списку каталогов: перед публикацией проверь,
что сам не добавил секреты в код, документацию или scripts. Символические ссылки
генератор отклоняет. Архив не содержит сторонние Cargo-зависимости: их скачает brew
на стадии fetch, затем install работает без сети.

SHA-256 в formula уже соответствует **этому** архиву. Не подставляй другой архив,
включая автоматические GitHub `Source code (tar.gz)` и готовый arm64-бинарник.
Одинаковые исходники и права файлов дают одинаковые архивы: даты и владельцы
нормализованы. После изменения любых включённых файлов запусти генератор снова.

## 2. Загрузить основной репозиторий

Загрузи исходники проекта в `YOUR_LOGIN/rmpkgx`, включая Cargo.lock, scripts,
docs, packaging и `.github`. Не загружай `target/` и `dist/` в историю Git — они
уже в `.gitignore`. Локальный git-репозиторий и тег автоматически не создаются.

В GitHub создай release `v0.1.0` для загруженного коммита. Прикрепи два файла
из `dist/homebrew/`: source.tar.gz и его .sha256. Описание релиза можно взять
из CHANGELOG.md. До публикации проверь ограничения и результаты испытаний;
публикация в brew не означает, что все сценарии настоящей macOS Trash проверены.

Workflow основного проекта может собрать те же файлы как Actions artifacts,
используя владельца репозитория. Он **не публикует** GitHub release и не меняет tap.
Если используешь артефакт workflow, бери source-архив и formula из одного запуска.

Проверь публичную загрузку после публикации:

```sh
curl --fail --location --output /tmp/rmpkgx-v0.1.0-source.tar.gz \
  https://github.com/YOUR_LOGIN/rmpkgx/releases/download/v0.1.0/rmpkgx-v0.1.0-source.tar.gz
shasum -a 256 /tmp/rmpkgx-v0.1.0-source.tar.gz
```

Хеш должен совпадать с `sha256` в сгенерированном `Formula/rmpkgx.rb`.
Не перемещай опубликованный тег и не заменяй архив другими байтами: исправления
выпускай новой версией.

## 3. Загрузить tap

В `YOUR_LOGIN/homebrew-tap` загрузи **содержимое**
`dist/homebrew/homebrew-tap/`. В корне репозитория должна оказаться `Formula/`,
а не ещё одна вложенная папка `homebrew-tap/`. Не забудь скрытую `.github/`.

Публикуй tap после того, как source-архив доступен по указанному URL.
Workflow tap установит formula из исходников и выполнит `brew test` на Apple Silicon.
GitHub Actions здесь не проверены запуском из локальной среды; после загрузки
проверь их результат в обоих репозиториях.

## 4. Проверить установку через Homebrew

На своём Mac после публикации:

```sh
brew tap YOUR_LOGIN/tap
brew trust --formula YOUR_LOGIN/tap/rmpkgx
brew install --build-from-source YOUR_LOGIN/tap/rmpkgx
brew test YOUR_LOGIN/tap/rmpkgx
rmpkgx --version
rmpkgx
```

Для старой версии Homebrew без `brew trust` сначала обнови Homebrew. Доверие
разрешает выполнять код formula; запрашивается только для одной formula, не всего tap.
На первом запуске установятся Rust и зависимости сборки. Во время обычной работы
rmpkgx сеть не использует. Bottles пока нет: сборка выполняется на компьютере пользователя.

В ограниченной локальной среде проекта запись в `/opt/homebrew/Cellar` может быть
запрещена. Изолированный Cargo-install из source-архива не заменяет этот настоящий
`brew install` и запуск workflow — их результат нужно проверить после загрузки.
`brew test` проверяет только version/help и корректный отказ без TTY, не удаляет приложения.

## 5. Обновления и удаление

Для следующего выпуска измени версию в Cargo.toml, обнови CHANGELOG.md и повтори
генерацию. Выложи новый source-архив с новым тегом, затем обнови formula в tap.
Не требуется подписка Apple или переустановка самого tap.

Пользователь обновляет утилиту:

```sh
brew update
brew upgrade YOUR_LOGIN/tap/rmpkgx
```

Удаление самой утилиты:

```sh
brew uninstall YOUR_LOGIN/tap/rmpkgx
```

Журналы в пользовательской Library при этом сохраняются.

Документация: [Homebrew taps](https://docs.brew.sh/How-to-Create-and-Maintain-a-Tap),
[Rust formulae](https://docs.brew.sh/Language-Specific-Formulae#rust).
