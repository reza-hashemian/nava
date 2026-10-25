<div dir="rtl">

# نوا (Nava)

[English](README.md) · **فارسی**

یک پخش‌کننده‌ی آرام و شیشه‌ای برای موسیقی و ویدیو، نوشته‌شده با Rust. رابط کاربری با Slint ساخته شده و پخش با libmpv انجام می‌شود.
مصرف رم و پردازنده‌اش خیلی کم است و یک «حالت کد» دارد: مینی‌پلیری که از داخل ترمینال کنترلش می‌کنید.

</div>

<p align="center">
  <img src="docs/screenshots/now-playing.png" alt="نوا: در حال پخش" width="860">
</p>

<p align="center">
  <img src="docs/screenshots/albums.png" alt="آلبوم‌ها" width="420">
  <img src="docs/screenshots/dark.png" alt="تم تیره" width="420">
</p>
<p align="center">
  <img src="docs/screenshots/equalizer.png" alt="اکولایزر" width="420">
  <img src="docs/screenshots/songs.png" alt="آهنگ‌ها" width="420">
</p>
<p align="center">
  <img src="docs/screenshots/mini.png" alt="مینی‌پلیر" width="440">
</p>

<div dir="rtl">

<sub>داده‌های داخل عکس‌ها ساختگی هستند.</sub>

## دانلود

| سیستم‌عامل | دانلود |
| --- | --- |
| **ویندوز** ۱۰ و ۱۱ (x64) | [نصب‌کننده (.exe)](https://github.com/reza-hashemian/nava/releases/latest/download/nava-windows-x64-setup.exe) · [نسخه‌ی پرتابل (.zip)](https://github.com/reza-hashemian/nava/releases/latest/download/nava-windows-x64.zip) |
| **مک** با تراشه‌ی اپل (M1 تا M4) | [nava-macos-arm64.dmg](https://github.com/reza-hashemian/nava/releases/latest/download/nava-macos-arm64.dmg) |
| **مک** اینتل | [nava-macos-x86_64.dmg](https://github.com/reza-hashemian/nava/releases/latest/download/nava-macos-x86_64.dmg) |
| **لینوکس**، هر توزیعی (x86_64) | [AppImage](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-x86_64.AppImage) |
| **دبیان / اوبونتو / مینت** (x86_64) | [.deb](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-x86_64.deb) |
| **فدورا / اوپن‌سوزه / RHEL** (x86_64) | [.rpm](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-x86_64.rpm) |
| **آرچ و بقیه** (x86_64) | [.tar.gz](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-x86_64.tar.gz) (باید `mpv` نصب باشد) |
| **لینوکس ARM64** (رزبری‌پای ۵ و …) | [AppImage](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-aarch64.AppImage) · [.deb](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-aarch64.deb) · [.rpm](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-aarch64.rpm) · [.tar.gz](https://github.com/reza-hashemian/nava/releases/latest/download/nava-linux-aarch64.tar.gz) |

همه‌ی نسخه‌ها: [Releases](https://github.com/reza-hashemian/nava/releases)

- **لینوکس:** بسته‌های `.deb` و `.rpm` به دبیان ۱۲ به بعد، اوبونتو ۲۴.۰۴ به بعد یا فدورا و اوپن‌سوزه‌ی جدید نیاز دارند. روی سیستم‌های قدیمی‌تر از AppImage استفاده کنید: اول `chmod +x` بزنید، بعد اجرایش کنید.
- **مک:** برنامه امضای Apple Developer ندارد. بار اول روی Nava راست‌کلیک کنید و **Open** را بزنید، یا این دستور را اجرا کنید: `xattr -dr com.apple.quarantine /Applications/Nava.app`
- **ویندوز:** ممکن است SmartScreen درباره‌ی ناشر ناشناس هشدار بدهد: **More info** و بعد **Run anyway**. اگر در نصب‌کننده گزینه‌ی «Add nava to PATH» را تیک بزنید، دستور `nava` در ترمینال هم کار می‌کند.
- برای پخش لینک (یوتیوب و سایت‌های دیگر) باید [yt-dlp](https://github.com/yt-dlp/yt-dlp) نصب باشد.

## حالت کد

`nava --code` جلسه‌ی Code را در یک مینی‌پلیر کوچک که همیشه روی بقیه‌ی پنجره‌ها می‌ماند باز می‌کند و از همان جایی که مانده بودید، با فید سه‌ثانیه‌ای، پخش را ادامه می‌دهد. اگر نوا از قبل باز باشد، فرمان به همان پنجره می‌رود و نسخه‌ی دوم باز نمی‌شود.

بدون اینکه از ادیتور بیرون بیایید کنترلش کنید:

</div>

```sh
nava toggle | play | pause | next | prev
nava mini | full | show
nava status                                  # ▶ Title — Artist  1:23 / 3:45
nava status --format "{state} {title} · {pos}/{dur}"   # برای tmux یا پرامپت
nava sleep 30 | end | off
nava speed 1.25
nava mark "اینجا closure رو توضیح داد"
```

<div dir="rtl">

وقتی نوا باز نباشد، `status` چیزی چاپ نمی‌کند (کد خروج ۱)، پس با خیال راحت در پرامپت استفاده‌اش کنید.

میان‌برهای سراسری (X11، ویندوز، مک):

| میان‌بر | کار |
| --- | --- |
| Ctrl+Alt+Space | پخش / توقف |
| Ctrl+Alt+. | بعدی |
| Ctrl+Alt+, | قبلی |
| Ctrl+Alt+M | مینی‌پلیر / پنجره‌ی کامل |

کلیدهای مدیا و ویجت «در حال پخش» دسکتاپ در لینوکس از طریق MPRIS کار می‌کنند (`playerctl` هم کار می‌کند)، و در مک و ویندوز از طریق souvlaki.

## کتابخانه

پوشه‌های موسیقی و ویدیوی خود را با دکمه‌ی ⊕ پایین نوار کناری اضافه کنید. نوا آن‌ها را در پس‌زمینه فهرست می‌کند، هر ده دقیقه دنبال فایل‌های تازه می‌گردد و فقط فایل‌هایی را که تغییر کرده‌اند دوباره می‌خواند.

- آهنگ‌ها، آلبوم‌ها (شبکه‌ی کاورها)، هنرمندان، ویدیوها، علاقه‌مندی‌ها، تازه‌اضافه‌شده‌ها
- جست‌وجوی همزمان با تایپ در عنوان، هنرمند، آلبوم و نام فایل
- قلب (یا کلید L) آهنگ را به علاقه‌مندی‌ها اضافه می‌کند
- با کلیک روی یک آهنگ، کل همان فهرست از آنجا پخش می‌شود؛ یا از Play و Shuffle استفاده کنید

## جلسه‌ها (Sessions)

جلسه‌های Main، Code و Relax همیشه هستند و با + کنار «SESSIONS» می‌توانید جلسه‌ی خودتان را بسازید. هر جلسه فهرست، جای پخش و صدای خودش را نگه می‌دارد.

با راست‌کلیک روی یک آهنگ گزینه‌های **Play Next**، **Show in Folder** و **Move to Trash** را دارید. فایل‌ها به سطل زباله‌ی سیستم می‌روند و قابل برگرداندن‌اند.

## متن ترانه

کلید Y (یا دکمه‌ی نقل‌قول) متن ترانه را کنار صفحه‌ی پخش نشان می‌دهد. متن‌های زمان‌دار مثل Apple Music همراه آهنگ جلو می‌روند و با کلیک روی هر خط، پخش به همان‌جا می‌پرد. نوا به این ترتیب دنبال متن می‌گردد: فایل `.lrc` کنار آهنگ، تگ‌های خود فایل، و بعد سایت [LRCLIB](https://lrclib.net) (جست‌وجوی آنلاین در تنظیمات قابل خاموش‌کردن است).

## اکولایزر

اکولایزر (کلید E) ده باند از ۳۱ هرتز تا ۱۶ کیلوهرتز دارد. نقطه‌ها را روی منحنی بکشید و برای ریست، دوبار کلیک کنید. ۱۹ پریست آماده دارد (Pop، Rock، Hip-Hop، EDM، Deep House، R&B، Jazz، Classical، Podcast، Bass Boost و…)، به‌علاوه‌ی پری‌امپ و پریست‌های شخصی.

حالت **Auto** برای هر آهنگ یک پریست انتخاب می‌کند: اگر ژانر در تگ باشد از روی آن، وگرنه از روی خود فایل. مثلاً فایل‌های مونو یا کم‌کیفیت و نام‌هایی مثل «جلسه» یا «voice» پریست Podcast می‌گیرند.

وقتی صفحه قفل می‌شود موسیقی متوقف می‌شود و بعد از باز کردن قفل ادامه پیدا می‌کند (گنوم و KDE).

## ابزارهای شنیدن

- **تایمر خواب** (دکمه‌ی ماه، یا `nava sleep 30|end|off`): در ۳۰ ثانیه‌ی آخر صدا را کم‌کم قطع می‌کند و بعد صدا را به حالت قبل برمی‌گرداند.
- **سرعت** (دکمه‌ی 1×، یا `nava speed 1.5`): از ۰.۵ تا ۲ برابر، بدون تغییر زیر و بمی صدا. هر پوشه سرعت خودش را یادش می‌ماند؛ دوره‌های آموزشی روی ۱.۵ برابر و موسیقی روی ۱ برابر.
- **تکرار A-B** (کلید A): بار اول نقطه‌ی A، بار دوم نقطه‌ی B و شروع تکرار، بار سوم پاک می‌شود.
- **یادداشت روی ویدیو** (کلید B، یا `nava mark "یادداشت"`): یادداشت لحظه‌ای بنویسید و با کلیک روی زمانش برگردید. «Export .md» همه را در یک فایل Markdown ذخیره می‌کند.

## لینک‌ها (یوتیوب و بیشتر)

با Ctrl+L یا `nava URL` لینک‌ها از طریق yt-dlp پخش می‌شوند. گزینه‌ی «Audio only» (`--audio-only`) تصویر را نمی‌گیرد و برای موسیقی و سخنرانی خیلی سبک‌تر است.

## ویدیو

ویدیوها در همان پنجره پخش می‌شوند و کنترل‌های شیشه‌ای به سبک QuickTime روی تصویر می‌آیند و بعد از ۲.۵ ثانیه محو می‌شوند. در حالت مینی، ویدیو به یک پنجره‌ی کوچک تصویر در تصویر تبدیل می‌شود.

- هر ویدیو جایی را که متوقف کرده‌اید یادش می‌ماند.
- زیرنویس‌های کنار ویدیو خودکار و با فونت وزیرمتن بار می‌شوند. فایل‌های `.srt` فارسی قدیمی با انکودینگ Windows-1256 هم درست خوانده می‌شوند.
- منوی زیرنویس و صدا، تمام‌صفحه (F یا دوبار کلیک)، و پنل Up Next.

## کلیدهای داخل پنجره

| کلید | کار |
| --- | --- |
| Space | پخش / توقف |
| ← / → | پنج ثانیه عقب / جلو |
| ↑ / ↓ | صدا |
| N / P | بعدی / قبلی |
| Ctrl+O | باز کردن پوشه |
| Ctrl+, | تنظیمات ظاهر (میزان شیشه‌ای بودن، تم) |
| Ctrl+M | مینی‌پلیر / پنجره‌ی کامل |
| Ctrl+L | پخش لینک |
| Delete | انتقال آهنگ در حال پخش به سطل زباله |
| L | علاقه‌مندی |
| Y | متن ترانه |
| E | اکولایزر |
| A | تکرار A-B |
| B | یادداشت‌ها (ویدیو) |
| F | تمام‌صفحه (ویدیو) |
| Esc | خروج از تمام‌صفحه / بستن پنجره‌ها |

## ساخت از سورس

به Rust و libmpv نیاز دارید (در دبیان و اوبونتو: `sudo apt install libmpv-dev`).

</div>

```sh
cargo run --release                      # باز کردن آخرین جلسه
cargo run --release -- --code            # حالت کد
cargo run --release -- ~/Music --limit 20 --pick random
cargo run --release -- --help
```

<div dir="rtl">

## مجوز

[MIT](LICENSE)

</div>
