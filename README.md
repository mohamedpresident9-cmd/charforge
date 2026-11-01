# CharForge Studio

استوديو شخصيات مكتوب بلغة **Rust** بالكامل: نقل أصول مُجزَّأ ومتوازٍ، مكتبة محلية موثَّقة بالبصمات، وواجهة سطح مكتب تعمل على **Windows و Linux**.

> **ما هذا وما ليس:** تطبيق جديد مستقل، وليس نسخة من `Fuse.exe` ولا تفريغًا لكوده. بُني على **المبادئ الهندسية** التي أثبتها تحليل ذلك الملف: طابور FIFO، حارس تنفيذ واحد لكل هدف، حل كسول للسجلات عند السحب، ومجموعة عمّال. أما تقسيم الحمولة إلى أجزاء (chunking) فهذه إضافة خاصة بنا لم تكن موجودة في الأصل.

[![CI](https://github.com/OWNER/charforge/actions/workflows/ci.yml/badge.svg)](../../actions/workflows/ci.yml)

## المنصّات

| المنصّة | المعمارية | الحزمة المنشورة |
| --- | --- | --- |
| Windows 10+ | x86_64 | `charforge-windows-x86_64.zip` → `charforge.exe` |
| Linux (X11/Wayland) | x86_64 | `charforge-linux-x86_64.tar.gz` → `charforge` |

الحزم تُبنى تلقائيًا في GitHub Actions عند كل وسم إصدار (`v*`) وتُرفَق بصفحة Releases؛ راجع `.github/workflows/release.yml`.

## التشغيل من الحزمة

- **Windows:** فُكّ الضغط وشغّل `charforge.exe`.
- **Linux:**
  ```bash
  tar -xzf charforge-linux-x86_64.tar.gz
  chmod +x charforge
  ./charforge
  ```
  يحتاج سطح مكتب يعمل بـ OpenGL: حزم التشغيل الشائعة `libxkbcommon0`,`libx11-6`,`libgl1`.

## البناء من المصدر

```bash
# المشترط: Rust 1.75+ (اختُبر على 1.99.1)
git clone <repo-url> charforge && cd charforge
cargo build --release
cargo test
```

- **Windows:** أداة البناء MSVC (Visual Studio Build Tools مع C++) كافية، ولا حاجة لأي حزمة إضافية.
- **Linux (Debian/Ubuntu):**
  ```bash
  sudo apt-get install -y libxkbcommon-dev libx11-dev libxi-dev \
      libxcursor-dev libxrandr-dev libgl1-mesa-dev
  ```

## أين تُحفظ المكتبة

| النظام | المسار |
| --- | --- |
| Windows | `%APPDATA%\CharForge\library` |
| Linux | `$XDG_DATA_HOME/charforge/library` أو `~/.local/share/charforge/library` |
| أي نظام (تجاوز) | متغيّر البيئة `CHARFORGE_LIBRARY` |

## الميزات

| الميزة | أين في الكود |
| --- | --- |
| فئات: بشر، حيوانات، سيارات، مباني | `model::AssetClass` |
| نوع الحمولة: مجسّم / أنميشن / ملابس | `model::AssetKind` |
| جودتان: منخفضة وعالية (×4 حجمًا) | `model::Quality` |
| تنزيل الأنميشن والملابس إلى الجهاز ثم استعمالها | `ui::Action::Download` + `Action::Attach` |
| نقل مُجزَّأ متوازٍ مع تحقّق لكل جزء | `transport::download` + `source.manifest` |
| دمج الأجزاء ثم التحقق من البصمة الكلية | `transport::download` |
| كتابة ذرّية على القرص + تحقّق عند السكون | `store::Store::install` |
| طابور مهامّ + single-flight + إعادة محاولة + إلغاء | `pipeline::Engine` |
| لوحة طابور تعرض `Jobs in queue: N` | `ui` (الشريط العلوي) |

## البنية

```
charforge/
├─ Cargo.toml
├─ .github/workflows/ci.yml        اختبار + بناء على النظامين
├─ .github/workflows/release.yml   حزم الإصدار على الوسم
└─ src/
   ├─ main.rs        نقطة الدخول + مسار المكتبة لكل نظام
   ├─ model.rs       الفئات، الجودات، الكتالوج، هندسة الأجزاء
   ├─ transport.rs   الـmanifest، المصدر التجريبي، التنزيل المتوازي، التحقق
   ├─ store.rs       المكتبة على القرص: index.tsv + كتابة ذرّية
   ├─ pipeline.rs    الطابور، العمّال، single-flight، المحاولات، الأحداث
   └─ ui.rs          واجهة egui: كتالوج، طابور، مكتبة، إرفاق
```

## تدفّق البيانات

```
زر Download
   │  Engine::enqueue(asset, quality)
   ▼
طابور FIFO  ── (single-flight: لا مهمتان لنفس الحمولة)
   │
   ▼  عامل حر
claim: job.state = Running          ← يمنع التنفيذ المزدوج
   │
   ├─ حل السجل الآن: catalog.get(asset)   ← كسول، لا يُبنى عند الإدراج
   ▼
Source::manifest → بصمة لكل جزء + بصمة كلية
   ▼
تنزيل الأجزاء على W عمّال متزامنين، كل جزء يُتحقَّق فورًا
   ▼
تجميع بالترتيب → تحقّق من البصمة الكلية
   ▼
Store::install: .part → fsync → تحقّق من القرص → rename ذرّي → تحديث الفهرس
   ▼
Event::Finished → الواجهة تُحدّث المكتبة وتُظهر زر Use
```

### لماذا لا يتلف الملف؟

1. **تحقّق لكل جزء** من الـmanifest قبل التجميع: جزء تالف = فشل المهمة.
2. **تحقّق كلي** بعد التجميع وقبل الكتابة.
3. **كتابة ذرّية**: `.part` → قراءة تحقق → `rename`؛ لا يظهر ملف نصف مكتوب في الفهرس.
4. **تحقّق عند السكون**: زر *Verify* يعيد حساب البصمة من القرص.
5. **حارس single-flight**: لا يعمل عاملان على نفس الحمولة.

## الاختبارات

```bash
cargo test
```

14 اختبارًا: تغطية الأجزاء للحمولة بدقة، ثبات المصدر التجريبي، رفض جزء تالف، احترام الإلغاء، بقاء المكتبة بعد إعادة الفتح، وكشف تلاعب القرص.

## النشر

```bash
git tag v0.1.0
git push origin v0.1.0
```

يبني الـworkflow حزمتي Windows و Linux ويرفعهما إلى Releases تلقائيًا.

## حدود معلومة

- **الواجهة بالإنجليزية**: `egui` لا يُشكّل النص العربي (لا bidi/ligatures). الحل المتين: Iced مع `cosmic-text`.
- **لا تفكيك صيغ ثلاثية الأبعاد ولا رسم مشهد**: المشروع يدير النقل والمكتبة والجدولة؛ نقطة الربط `store::StoredAsset::path`.
- **مصدر تجريبي محلي** (`DemoSource`) يولّد بايتات حتمية ليعمل التطبيق بلا شبكة؛ ربط CDN حقيقي = تنفيذ دالّتين في `transport::Source`.
- **`index.tsv` بسيط عمدًا**؛ الاستبدال بـ SQLite ممكن دون مساس ببقية الطبقات.

## الترخيص

MIT — انظر [LICENSE](LICENSE).
