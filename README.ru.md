# imsforge

Включает **VoLTE, VoWiFi и VoNR** на телефонах Pixel для операторов, которых Google не
сертифицировал — патчит protobuf-файлы CarrierSettings и упаковывает их в модуль
KernelSU/Magisk.

В отличие от [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch), после перезагрузки
ничего нажимать не нужно: конфиг уже правильный к моменту старта телефонии, без приложения и
без Shizuku.

*[English version](README.md)*

---

## Почему VoLTE не работает изначально

Pixel берёт carrier config не из AOSP-приложения `com.android.carrierconfig`, а из
гугловского `com.google.android.carrier`, которое читает protobuf-файлы из
`/product/etc/CarrierSettings/`. Оператор находится по MCCMNC через `carrier_list.pb`, дальше
настройки берутся либо из `<canonical_name>.pb`, либо из общей пачки `others.pb`.

У несертифицированного оператора там лежат **только APN, а блок `configs` пустой**. Поэтому
`carrier_volte_available_bool` остаётся `false`, IMS не регистрируется, и звонки уходят через
CSFB на 2G/3G — что становится проблемой по мере отключения 3G.

Есть и вторая половина, которую обычно упускают утилиты, просто переключающие флаги: у таких
операторов, как правило, **нет APN типа IMS**. Без него IMS-PDN не поднимается, оператор не
выдаёт P-CSCF (адрес SIP-прокси), и регистрация не может даже начаться. imsforge добавляет APN
тем же патчем.

## Требования

- Pixel (или другое устройство, использующее гугловский CarrierSettings) с root: KernelSU,
  KernelSU Next, APatch или Magisk.
- **Метамодуль монтирования.** Начиная с KernelSU 3.x менеджер сам файлы модулей больше не
  монтирует — эта логика вынесена в подключаемый бэкенд. Без него модуль, доставляющий файлы,
  молча ничего не делает: он числится установленным, его скрипты выполняются, а до файловой
  системы ничего не доходит. Поставь один из:
  [NoMount](https://github.com/maxsteeel/nomount) (редирект путей на уровне VFS, не оставляет
  следов в `/proc/mounts`; требует `CONFIG_NOMOUNT=y` в ядре),
  [Mountify](https://github.com/backslashxx/mountify) (OverlayFS, работает на любом ядре) или
  [meta-overlayfs](https://github.com/KernelSU-Modules-Repo/meta-overlayfs).
  Команда `imsforge verify` сообщит, если метамодуля нет.
- `adb` с root-доступом на устройстве и [uv](https://docs.astral.sh/uv/).

## Быстрый старт

```bash
uv run imsforge carriers   # определить canonical-имена для симок в телефоне
$EDITOR carriers.toml      # вписать их
uv run imsforge pull       # снять стоковый CarrierSettings (модуль должен быть выключен!)
uv run imsforge build      # пропатчить protobuf, собрать dist/imsforge.zip
uv run imsforge install    # залить и поставить через ksud
# перезагрузка
uv run imsforge verify     # проверить, что всё реально применилось
```

`dist/imsforge.zip` — обычный zip модуля, его можно поставить и вручную из менеджера
KernelSU/Magisk вместо `imsforge install`.

Есть ещё `uv run imsforge apply` — применяет пропатченный конфиг сразу, без перезагрузки
(временный bind-mount + сброс кэша + рестарт телефонии; связь пропадает на несколько секунд).
Удобно при отладке и как запасной путь, если бэкенд монтирования когда-нибудь отвалится.

## Настройка операторов

Список живёт в `carriers.toml`. Обязательное поле одно — canonical-имя оператора, тот самый
идентификатор, которым Google оперирует внутри CarrierSettings:

```toml
[[carrier]]
canonical_name = "25001"      # безымянный оператор: canonical-имя это просто MCCMNC
ims_apn_name = "MTS IMS"

[carrier.int_arrays]
carrier_nr_availabilities_int_array = [1, 2]   # 1 = NSA, 2 = SA, то есть ещё и 5G SA
```

`imsforge carriers` выведет правильное имя для тех симок, что сейчас в телефоне, включая МВНО
с матчингом по SPN/IMSI/GID1, и заодно готовый блок для копирования. Необязательные ключи
(`vowifi`, `ims_apn`, `ims_apn_value`, `[carrier.bools]`) описаны комментариями прямо в файле.

Набор ключей повторяет то, что выставляет PixelIMS, минус
`carrier_supports_ss_over_ut_bool` — он ломает переадресацию, если у оператора не работает
XCAP.

## Как это устроено

1. `pull` снимает `others.pb`, все `<canonical_name>.pb` и отпечаток сборки в `stock/`.
2. `build` разбирает их по схемам из AOSP, заполняет блок `configs`, добавляет IMS-APN,
   инкрементирует поля версии (чтобы пропатченный конфиг было видно в
   `dumpsys carrier_config` как `carrier_config_version_string`) и собирает zip модуля.
3. На загрузке бэкенд монтирования кладёт пропатченные файлы поверх
   `/product/etc/CarrierSettings/`, а `post-fs-data.sh` модуля удаляет кэш carrier config.
   Это удаление обязательно: телефония инвалидирует кэш по *версии APK конфиг-приложения*, а
   не по версии protobuf-данных, поэтому иначе пропатченные файлы просто не будут прочитаны.
   Скрипт выполняется до старта `system_server`, так что телефония поднимается уже на новом
   конфиге.

## Проверка и диагностика

`imsforge verify` проходит всю цепочку и показывает, где она рвётся:

| Симптом | Что значит |
|---|---|
| `NO metamodule installed` | Нет бэкенда монтирования — модуль пустышка. См. «Требования». |
| `others.pb: … (STOCK — not being shadowed)` | Бэкенд файлы не применил. |
| Только `carrier_volte_available_bool: false` | Телефония на старом кэше; проверь, отработал ли `post-fs-data.sh` (`last-boot.log`). |
| Нет IMS-PDN / `P-CSCF: NONE` | IMS-APN не подключился — сторона оператора либо неверное значение APN. |

Два сценария патчем **не лечатся**, и их стоит различать, прежде чем заводить баг:

- **`mVopsSupport = 3`** в `dumpsys telephony.registry` означает, что сама сеть не предлагает
  этой SIM голос по LTE (`2` — предлагает). Никакой carrier config это не переопределит.
- **`IWLAN_IKEV2_AUTH_FAILURE`** в логе означает, что ePDG оператора ответил на твой
  VoWiFi-туннель и *отклонил аутентификацию* — услуга на номере не подключена.

И то, и другое — повод обратиться к оператору, а не к патчу.

Про батарею: если «Звонки по Wi-Fi» включены для симки, у оператора которой ePDG не работает,
Android будет бесконечно пересоздавать туннель примерно раз в 20 секунд. Выключи тумблер для
этой симки в настройках или поставь ей `vowifi = false`.

## После обновления системы

OTA перезаписывает `/product`, и модуль начнёт подменять свежие данные Google старым снимком.
Пересобрать:

1. отключить модуль в менеджере, перезагрузиться;
2. `uv run imsforge pull` — снять новый сток;
3. `uv run imsforge build && uv run imsforge install`, включить модуль, перезагрузиться.

`customize.sh` сверяет стоковый `others.pb` со снимком, из которого собран модуль, и
предупреждает при расхождении; `pull` ругается, если снимок делается при активном модуле —
иначе в «сток» попадёт пропатченный файл.

## Структура

```
carriers.toml     операторы — единственный файл, который обычно нужно править
proto/            схемы из AOSP (platform/tools/carrier_settings)
stock/            снимок с устройства + stock.json с отпечатком сборки
module/           шаблон модуля (module.prop, post-fs-data.sh, uninstall.sh)
dist/             собранный модуль и zip
src/imsforge/
  protos.py       генерация биндингов на лету через grpcio-tools
  patch.py        правка protobuf: ключи конфига, IMS APN, int-массивы
  build.py        carriers / pull / build / install / apply / verify
```

## Ограничения

- `.pb` привязаны к конкретной сборке Android, поэтому модуль собирается под устройство и
  переустанавливается после OTA. `customize.sh` откажется ставиться на другое устройство.
- `vonr_enabled_bool` что-то даёт только там, где у оператора действительно есть 5G SA.
- Проверено на Pixel 8 Pro (husky), Android 17, KernelSU Next и NoMount. Подход не завязан на
  конкретную модель, но проверялось именно на ней.

## Благодарности

- [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch) — приложение, решающее ту же
  задачу в рантайме, и источник набора ключей carrier config.
- [carriersettings-extractor](https://github.com/GrapheneOS-Archive/carriersettings-extractor)
  — подсказал, где лежат protobuf-схемы AOSP.
- [AOSP platform/tools/carrier_settings](https://android.googlesource.com/platform/tools/carrier_settings/)
  — сами схемы.
- [NoMount](https://github.com/maxsteeel/nomount),
  [Mountify](https://github.com/backslashxx/mountify),
  [WildKernels](https://github.com/WildKernels/GKI_KernelSU_SUSFS) — бэкенды монтирования и
  ядра, которые их несут.

## Лицензия

MIT — см. [LICENSE](LICENSE).
