# imsforge

Включает **VoLTE, VoWiFi и VoNR** на телефонах Pixel для операторов, которых Google не
сертифицировал.

imsforge — модуль KernelSU/Magisk, который патчит protobuf-файлы CarrierSettings **на самом
устройстве, при каждой загрузке**. Не нужен компьютер, не нужна сборка под конкретный телефон,
не нужно ничего переделывать после обновления системы — и, в отличие от рантайм-утилит вроде
[PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch), не нужно ничего нажимать после
перезагрузки.

*[English version](README.md)*

---

## Почему VoLTE не работает изначально

Pixel берёт carrier config не из AOSP-приложения `com.android.carrierconfig`, а из гугловского
`com.google.android.carrier`, которое читает protobuf-файлы из `/product/etc/CarrierSettings/`.
Оператор находится по MCCMNC через `carrier_list.pb`, дальше настройки берутся либо из
`<canonical_name>.pb`, либо из общей пачки `others.pb`.

У несертифицированного оператора там лежат **только APN, а блок `configs` пустой**. Поэтому
`carrier_volte_available_bool` остаётся `false`, IMS не регистрируется, и звонки уходят через
CSFB на 2G/3G — что становится реальной проблемой по мере отключения 3G.

Есть и вторая половина, которую обычно упускают утилиты, просто переключающие флаги: у таких
операторов, как правило, **нет APN типа IMS**. Без него IMS-PDN не поднимается, оператор не
выдаёт P-CSCF (адрес SIP-прокси), и регистрация не может даже начаться. imsforge добавляет APN
тем же патчем.

## Требования

- Pixel (или другое устройство, использующее гугловский CarrierSettings) с root: KernelSU,
  KernelSU Next, APatch или Magisk.
- **Бэкенд монтирования.** У Magisk он встроен. Начиная с KernelSU 3.x менеджер сам файлы
  модулей больше не монтирует — это вынесено в подключаемый «метамодуль», и без него модуль,
  доставляющий файлы, молча ничего не делает: он числится установленным, его скрипты
  выполняются, а до файловой системы ничего не доходит. Поставь
  [NoMount](https://github.com/maxsteeel/nomount) (редирект путей на уровне VFS, не оставляет
  следов в `/proc/mounts`; требует `CONFIG_NOMOUNT=y` в ядре),
  [Mountify](https://github.com/backslashxx/mountify) (OverlayFS, любое ядро) или
  [meta-overlayfs](https://github.com/KernelSU-Modules-Repo/meta-overlayfs). WebUI модуля прямо
  сообщает, если бэкенда нет.

## Установка

1. Поставить `imsforge.zip` в менеджере root.
2. Перезагрузиться.

Это вся процедура. Операторы вставленных симок определяются и патчатся автоматически, а тех,
кого Google уже сертифицировал, модуль намеренно не трогает: у них выверенный конфиг, и
перезапись его — рабочий способ сломать работающий VoLTE.

## WebUI

Интерфейс модуля открывается из менеджера (или через
[KsuWebUI](https://github.com/a13e300/KsuWebUI)) и показывает прямо на телефоне:

- есть ли бэкенд монтирования и дошёл ли патч до `/product`;
- во что определилась каждая симка — пропатчена, пропущена или неизвестна;
- какой carrier config в итоге использует телефония, состояние IMS-PDN и адрес P-CSCF;
- переопределения — добавить, отредактировать, удалить, плюс редактор сырого JSON.

Отдельно интерфейс объясняет два состояния, которые патчем не лечатся, чтобы они не выглядели
как баг модуля:

- **`mVopsSupport = 3`** — сеть не предлагает этой SIM голос по LTE (`2` — предлагает). Никакой
  carrier config это не переопределит.
- **`IWLAN_IKEV2_AUTH_FAILURE`** — ePDG оператора ответил на VoWiFi-туннель и *отклонил
  аутентификацию*: услуга на номере не подключена.

И то, и другое — повод обратиться к оператору.

## Переопределения

Необязательны и нужны только для частностей: нестандартный IMS-APN, дополнительные ключи
конфига или принудительное включение оператора, которого автоопределение пропустило. Правятся в
WebUI либо руками в `/data/adb/modules/imsforge/carriers.json` — см.
[carriers.example.json](carriers.example.json).

```json
{
  "carriers": [
    {
      "canonical_name": "25001",
      "ims_apn_name": "MTS IMS",
      "int_arrays": { "carrier_nr_availabilities_int_array": [1, 2] }
    }
  ]
}
```

`canonical_name` — идентификатор, которым Google оперирует внутри CarrierSettings; у безымянного
оператора это просто MCCMNC. WebUI показывает правильное имя для каждой вставленной симки, а
`imsforge detect` печатает его в JSON. Остальные ключи: `ims_apn_value`, `ims_apn: false`,
`bools`, `int_arrays`, плюс `auto: false` и `skip: ["имя"]` на верхнем уровне.

Набор ключей повторяет то, что выставляет PixelIMS, минус `carrier_supports_ss_over_ut_bool` —
он ломает переадресацию, если у оператора не работает XCAP.

## Как это устроено

При каждой загрузке `post-fs-data.sh` запускает патчер **до того, как бэкенд монтирования
положит файлы модуля поверх системных**. Оба root-решения этот порядок документируют — Magisk:
*«Scripts run before any modules are mounted. This allows a module developer to dynamically
adjust their modules before it gets mounted.»* Значит в этот момент в `/product` лежат
оригиналы Google, и патч выводится из того, что приехало именно этой загрузкой. Поэтому
обновление системы в принципе не может оставить устаревший снимок.

Дальше патчер:

1. определяет вставленные симки и резолвит их в canonical-имена через `carrier_list.pb`, МВНО —
   по SPN;
2. пропускает операторов, у которых в стоке уже стоит `carrier_volte_available_bool = true`;
3. заполняет блок `configs`, добавляет IMS-APN, инкрементирует версию (чтобы результат было
   видно как `carrier_config_version_string` в `dumpsys carrier_config`);
4. пишет в собственный каталог модуля и переставляет файлам контекст `system_file` — файлы,
   созданные в `/data/adb`, наследуют метку, с которой конфиг-приложение их не прочитает;
5. удаляет кэш carrier config. Это обязательно: телефония инвалидирует кэш по *версии APK
   конфиг-приложения*, а не по версии protobuf-данных, поэтому иначе пропатченные файлы просто
   не будут прочитаны.

Копия стоковых исходников остаётся в `stock/` рядом с модулем вместе с отпечатком того, что было
сгенерировано. При ручном запуске в `/product` лежит уже собственный вывод imsforge, а не файлы
Google — читая его, проверка «сертифицирован ли оператор» приняла бы нашу же работу за гугловскую
и всё пропустила. Отпечаток эти два случая различает, и тогда берётся кэш. Прогон, который ничего
не изменил, считается чтением самого себя и кэш не обновляет.

Для правки protobuf взят [rust-protobuf](https://github.com/stepancheg/rust-protobuf) именно
потому, что он сохраняет поля, которых нет в нашей схеме. Google может добавить поле в
CarrierSettings в любой момент, а библиотека, которая неизвестные поля выбрасывает (prost,
quick-protobuf), молча потеряла бы их для всех остальных операторов в файле.

## Сборка

Нужны Android NDK, `cargo-ndk` и таргет `aarch64-linux-android`:

```bash
rustup target add aarch64-linux-android
cargo install cargo-ndk
export ANDROID_NDK_HOME=~/Android/Sdk/ndk/<версия>
./build.sh          # -> dist/imsforge.zip
```

В zip нет данных операторов, поэтому одна сборка подходит любому устройству.

## Структура

```
native/           патчер (Rust)
proto/            схемы CarrierSettings из AOSP
module/           шаблон модуля: скрипты, module.prop, webroot/
build.sh          кросс-сборка и упаковка dist/imsforge.zip
```

## Ограничения

- Имеет смысл только там, где carrier config поставляет гугловский CarrierSettings — Pixel и
  устройства с тем же приложением.
- `vonr_enabled_bool` что-то даёт только там, где у оператора действительно есть 5G SA.
- МВНО определяются по MCCMNC и SPN. Те, что различаются только префиксом IMSI или GID1,
  свалятся в общую запись — если это не твой случай, добавь переопределение явно.
- Проверено на Pixel 8 Pro (husky), Android 17, KernelSU Next с NoMount. Подход не завязан на
  конкретную модель, но проверялось именно на ней.

## Благодарности

- [PixelIMS](https://github.com/kyujin-cho/pixel-volte-patch) — решает ту же задачу в рантайме и
  является источником набора ключей carrier config.
- [carriersettings-extractor](https://github.com/GrapheneOS-Archive/carriersettings-extractor) —
  подсказал, где лежат protobuf-схемы AOSP.
- [AOSP platform/tools/carrier_settings](https://android.googlesource.com/platform/tools/carrier_settings/)
  — сами схемы.
- [NoMount](https://github.com/maxsteeel/nomount),
  [Mountify](https://github.com/backslashxx/mountify),
  [WildKernels](https://github.com/WildKernels/GKI_KernelSU_SUSFS) — бэкенды монтирования и
  ядра, которые их несут.

## Лицензия

MIT — см. [LICENSE](LICENSE).
