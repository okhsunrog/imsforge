# Отладка imsforge на устройстве

Инструкция для Pixel с KernelSU Next и ZeroMount VFS. Команды ZeroMount сверены
с установленным `v2.0.216-dev`, а `ksud profile` — с нашим форком KernelSU Next.
У других версий сначала проверь `--help`. Разделы про `apply`, статус format 2,
`read-config` и `save-config` относятся к исправленной сборке imsforge поверх исходного коммита 2.2.1.

Главное: **генерация патча, установка файлов модуля и видимость этих файлов для
процесса — три разных факта**. Успешный `patch` не доказывает регистрацию IMS.
Stock-файл в обычном `adb shell` не доказывает, что telephony получает тот же файл.

## Подключение и идентичность процесса

На компьютере:

```sh
adb devices
adb shell id
adb shell "su -c 'id; readlink /proc/self/ns/mnt'"
adb shell "su -M -c 'id; readlink /proc/self/ns/mnt'"
```

Если устройств несколько, добавляй `-s SERIAL` после каждого `adb`. Обычный shell
имеет UID 2000; `su` обычно выполняет команду с UID 0. `su -M` выбирает глобальный
mount namespace. Всегда смотри `id`: профиль KernelSU способен изменить UID,
группы, SELinux domain и namespace root-команды.

Важны **оба уровня кавычек**. Пиши `adb shell "su -c 'команда; другая команда'"`.
Иначе вторая команда может выполниться обычным shell, а не через `su`, и ошибка
`Permission denied` будет ошибочно принята за неисправность ядра/CLI.

## Что установлено и что сделал текущий запуск

```sh
adb shell "su -c 'cat /data/adb/modules/imsforge/module.prop'"
adb shell "su -c '/data/adb/modules/imsforge/bin/imsforge --version'"
adb shell "su -c '/data/adb/modules/imsforge/bin/imsforge read-config'"
adb shell "su -c '/data/adb/modules/imsforge/bin/imsforge detect'"
adb shell "su -c '/data/adb/modules/imsforge/bin/imsforge status'"
adb shell "su -c 'cat /data/adb/modules/imsforge/last-boot.log'"
adb shell "su -c 'cat /data/adb/imsforge/last-detect.log'"
```

В `status`:

- `current_boot` говорит, совпадает ли boot_id отчёта с текущей загрузкой;
- `run.phase=generated` — только генерация; `preparing` — начатая установка;
  `applied` — файлы установлены, `failed` — операция закончилась ошибкой;
- `run.error` объясняет ошибку; старое успешное выполнение не заменяет её;
- `run.carriers` содержит решения по операторам, `run.files` — ожидаемые файлы;
- `config_changed=true` означает, что сохранённая конфигурация отличается от
  конфигурации записанного запуска;
- `matches_run` проверяет байты ожидаемых файлов **из контекста наблюдателя**;
- `product=ours` означает совпадение с известным сгенерированным результатом,
  который может быть старее текущей сохранённой конфигурации.

Отчёт format 1 от 2.2.1 не содержит boot_id и не доказывает применение в текущей
загрузке. В `detect` используй `slot`, а не индекс элемента массива. `complete=false`
означает, что инвентаризация SIM ещё не полна; `service.sh` сохраняет инвентарь
только при подтверждённой готовности слотов.

## Где находятся оригиналы и подмены

```sh
adb shell "su -c 'cat /data/adb/imsforge/layout'"
adb shell "su -c 'ls -lZ /data/adb/modules/imsforge/product/etc/CarrierSettings'"
adb shell "su -c 'ls -lZ /data/adb/modules/imsforge/system/product/etc/CarrierSettings'"
adb shell "su -c 'ls -l /data/adb/imsforge/stock'"
```

Один из двух путей модуля может отсутствовать. Используй обнаруженный layout;
не угадывай его только по названию root-менеджера.

Сравни байты, например для `others.pb` и layout `product`:

```sh
adb shell "su -c 'sha256sum /data/adb/imsforge/stock/others.pb /data/adb/modules/imsforge/product/etc/CarrierSettings/others.pb /product/etc/CarrierSettings/others.pb'"
adb shell 'sha256sum /product/etc/CarrierSettings/others.pb'
adb shell "su -M -c 'sha256sum /product/etc/CarrierSettings/others.pb'"
```

Смысл трёх файлов: сохранённый stock → результат модуля → файл, который видит
данная команда по системному пути. Для оператора с отдельным protobuf проверяй
также `<canonical_name>.pb`. Совпадение одного `others.pb` не проверяет все файлы.

Не используй только `stat`, inode, размер или `/proc/mounts`: ZeroMount работает
через VFS, а SUSFS/kstat redirect могут маскировать метаданные. Сравнивай содержимое.
Кэш stock — локальный снимок, его происхождение тоже важно; это не независимый
эталон, если его вручную подменили.

## ZeroMount: правила и видимость для UID

Установленный на этом устройстве модуль называется `meta-zeromount`:

```sh
adb shell "su -c 'readlink /data/adb/metamodule'"
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm version'"
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm status'"
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm vfs query-status'"
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm vfs list'"
```

`vfs list` показывает фактические правила, в том числе подмену
`.../imsforge/product/etc/CarrierSettings/others.pb` на системном пути. Наличие
правила не доказывает, что оно применяется ко всем процессам.

В этой версии есть **список исключённых UID**, а не команда выдачи разрешения:

```sh
# Включить UID 2000 в VFS-перенаправление: убрать из исключений.
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm uid unblock 2000'"

# После этого открыть файл именно обычным shell, чей UID равен 2000.
adb shell 'id; sha256sum /product/etc/CarrierSettings/others.pb'

# Исключить UID 2000 из VFS-перенаправления.
adb shell "su -c '/data/adb/modules/meta-zeromount/bin/zm uid block 2000'"
adb shell 'id; sha256sum /product/etc/CarrierSettings/others.pb'
```

Это **изменяющие состояние** команды, применяй их при целенаправленной проверке.
Перед проверкой зафиксируй нужную исходную политику для выбранного UID и верни её
после проверки. `block` не является универсальной командой восстановления!
У данной CLI нет `uid list`; ошибка при повторном добавлении/удалении не означает
поломку драйвера. Обработчики этих команд меняют таблицу ядра, а не сохраняют
настройку в TOML, поэтому не рассчитывай на сохранение изменения после перезагрузки.

Меняй только диагностический UID. Не переключай весь движок через `vfs disable`,
не очищай правила через `vfs clear` и не исключай telephony ради сравнения двух
хешей: это влияет на другие приложения и модули.

У ZeroMount есть ещё проверка SUSFS `susfs_is_current_proc_umounted()`. Поэтому
`uid unblock` **не гарантирует подмену**, если процесс уже помечен как не видящий
модули. Профиль KernelSU, метка процесса и namespace — отдельные причины, почему
результат может не измениться. `su -M` меняет namespace, но сам по себе не снимает
метку SUSFS и не очищает список исключений ZeroMount.

## KernelSU Next: профили через ksud

В нашем форке GET/SET_APP_PROFILE доступны root напрямую. Нужны и подходящий
`ksud`, и ядро с этим изменением; на несовместимом ядре CLI должен выдать ошибку,
а не притворяться приложением-менеджером.

```sh
adb shell "su -c '/data/adb/ksud profile get com.android.shell --uid 2000'" > shell-profile.before.json
adb shell "su -c '/data/adb/ksud profile get --help'"
adb shell "su -c '/data/adb/ksud profile set --help'"
```

Не меняй `allow_su` у shell на false ради эксперимента с монтированием: можно
потерять root через ADB. Root-профиль имеет `root.namespace` (`inherited`,
`global`, `individual`). Non-root профиль имеет `non_root.use_default` и
`non_root.umount_modules`. Эти половины не взаимозаменяемы; поле
`umount_modules` не нужно добавлять в root-профиль.

Для разовой проверки глобального namespace обычно хватает `su -M`, без изменения
сохранённого профиля. Если нужен именно профиль, создай изменённую копию на хосте,
сохранив все остальные поля. Например, только для уже разрешённого root-профиля:

```sh
uv run python - <<'PY'
import json
from pathlib import Path
p = json.loads(Path('shell-profile.before.json').read_text())
assert p['allow_su'] is True
p['root']['use_default'] = False
p['root']['namespace'] = 'global'
Path('shell-profile.test.json').write_text(json.dumps(p, ensure_ascii=False))
PY
adb shell "su -c '/data/adb/ksud profile set -'" < shell-profile.test.json
adb shell "su -c 'id; readlink /proc/self/ns/mnt; sha256sum /product/etc/CarrierSettings/others.pb'"

# Восстановить сохранённый профиль и открыть новую команду su.
adb shell "su -c '/data/adb/ksud profile set -'" < shell-profile.before.json
```

Смена профиля не доказывает изменение уже работающих процессов. Открой новый shell;
для приложения потребуется новый процесс. Не вызывай глобальный refresh меток
всех процессов лишь ради диагностики одного UID. `ksud debug mark` существует,
но это не универсальная команда управления скрытием модулей — проверь её смысл
в конкретном ядре перед использованием.

Для чтения с другим Linux UID можно использовать поддерживаемый здесь синтаксис
`su -M -c 'id; ...' NUMERIC_UID`. Это полезный контроль, но **не точная копия
контекста работающего приложения**: его namespace, SELinux domain и метки процесса
могут отличаться. Текущий UID пакета найди через `cmd package list packages -U`;
не подставляй номер приложения из старой сессии или другого Android-профиля.

## Проверка телеметрии и ошибок WebUI

```sh
adb shell "su -c 'sh /data/adb/modules/imsforge/probe.sh'" > imsforge-probe.txt
adb shell "su -c 'dumpsys carrier_config'" > carrier-config.txt
adb shell "su -c 'dumpsys telephony.registry'" > telephony-registry.txt
```

В probe у каждого раздела есть `<name>_ok` (код выхода) и `<name>_error`.
Ненулевой код — ошибка получения данных, не false/пустая конфигурация.
carrier_config смотри в разделе нужного `Phone Id`, отдельно от глобальных
defaults и исторического лога. Значение VoLTE у другой SIM ничего не доказывает.

P-CSCF — адрес IMS SIP-прокси. Он не доказывает регистрацию, голосовую capability
или звонок по LTE. Регистрация через IWLAN относится к Wi-Fi calling. Старый
`isImsRegistered` из logcat не является надёжным текущим состоянием. Полную проверку
делай отдельно: актуальная регистрация IMS, выбранная технология и реальный звонок
на нужной SIM, с понятным состоянием Wi-Fi и мобильной сети.

Для записи новой сессии логов запусти отдельную отслеживаемую фоновую задачу:

```sh
adb logcat -c && adb logcat
```

В интерактивном терминале заверши Ctrl-C; в среде с background task — остановкой
этой задачи. Не используй `adb logcat -d` как запись сессии и не запускай запись
через неотслеживаемый `&`. Дампы могут содержать идентификаторы SIM и другие личные
данные: перед прикреплением к issue оставляй только нужные обезличенные строки.

## Безопасная генерация для сравнения

`patch` генерирует отдельный результат и не устанавливает его. Выбери свободный
каталог: уже существующий каталог вывода будет заменён целиком.

```sh
adb shell "su -c '/data/adb/modules/imsforge/bin/imsforge patch --out /data/local/tmp/imsforge-check'"
adb shell "su -c 'cat /data/local/tmp/imsforge-check/.imsforge.json'"
adb shell "su -c 'sha256sum /data/local/tmp/imsforge-check/others.pb'"
```

Если текущий `/product` распознан как известная подмена, патчер использует stock
из своего кэша. Читай `source` в отчёте; неизвестное стороннее содержимое нельзя
автоматически считать оригиналом Google. Ручная генерация не записывает глобальный
статус успешной загрузки. `apply` предназначен для post-fs-data до монтирования:
не запускай его вручную на работающем телефоне ради чтения состояния.
