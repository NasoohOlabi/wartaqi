use super::{Language, clean_words, matches};

pub(super) fn canned(question: &str, lang: Language) -> Option<&'static str> {
    let question = clean_words(question);
    if matches(
        r#"^(hi|hello|hey|salam|مرحبا|اهلا|هلا|السلام عليكم|سلام|صباح الخير|مسا الخير)$"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Hello! Ask me about using Wartaq, or open /menu to get started."#,
            r#"أهلاً! اسألني عن استخدام وارتق، أو افتح /menu لنبدأ."#,
        ));
    }
    if matches(
        r#"\b(privacy|who can (?:see|read) my (?:questions|messages))\b|خصوصي|مين بيشوف اسئلتي"#,
        &question,
    ) {
        return Some(lang.text(
            r#"The assistant uses the text of some questions to choose where to look. Leave out passwords, codes and phone numbers. Students can only see their own attendance."#,
            r#"يستخدم المساعد نص بعض الأسئلة ليحدد أين يبحث. لا ترسل كلمات المرور أو الرموز أو أرقام الهاتف. الطالب يشوف سجل حضوره فقط."#,
        ));
    }
    if matches(
        r#"^(what can (?:you|this bot) do|help(?: me)?|شو بتقدر تعمل|شو فيني اعمل)$"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Ask how to sign in or use Wartaq. If you’re a student, try “Show my attendance”. Staff can list courses or search for a student. Open /menu for more options."#,
            r#"اسأل كيف تسجّل الدخول أو تستخدم وارتق. إذا كنت طالباً، جرّب «أرني سجل حضوري». يمكن للمعلمين والمسؤولين عرض الدورات أو البحث عن طالب. افتح /menu للمزيد."#,
        ));
    }
    if matches(r#"\bcancel\b|الغي الخطوه|الغي العمليه|كيف بلغي"#, &question)
    {
        return Some(lang.text(
            r#"Send /cancel to leave the current step, then /menu to choose another task."#,
            r#"أرسل /cancel للخروج من الخطوة الحالية، ثم /menu لاختيار مهمة ثانية."#,
        ));
    }
    if matches(
        r#"^(?:please )?(?:mark|record)\b.*\b(?:present|absent|excused)\b|^(?:سجل|حط)\s.*(?:حاضر|غائب|غايب|معذور)"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Open /menu → Take attendance, choose the course and date, then tap the mark next to the student’s name. The list shows the saved mark."#,
            r#"افتح /menu ثم تسجيل الحضور. اختر الدورة والتاريخ، واضغط الحالة بجانب اسم الطالب. تظهر العلامة المحفوظة في القائمة."#,
        ));
    }
    if matches(
        r#"\b(show|list|who|what is my|what are my|give me)\b|مين|من غاب|من حضر|اعرض|اريني|شو حضوري"#,
        &question,
    ) {
        return None;
    }
    if matches(r#"\b(code|codes)\b|كود|رمز"#, &question)
        && matches(r#"\b(expired|expire|expires)\b|انته|خلص|صلاحي"#, &question)
    {
        return Some(lang.text(
            r#"A link code lasts five minutes. Send /cancel then /linkaccount. Sign in and approve again, then send the new eight-digit code by itself in this same private chat."#,
            r#"رمز الربط صالح لخمس دقائق. أرسل /cancel ثم /linkaccount. سجّل الدخول ووافق من جديد، ثم أرسل الرمز الجديد من ثمانية أرقام وحده في نفس المحادثة الخاصة."#,
        ));
    }
    if matches(
        r#"\bwhat does excused mean\b|شو يعني معذور|ما معني معذور"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Excused means the teacher marked the absence as excused. Wartaq counts it separately from Present and Absent."#,
            r#"معذور يعني أن المعلم سجّل غيابك بعذر. وارتق يحسبه بشكل منفصل عن حاضر وغائب."#,
        ));
    }
    if !matches(
        r#"\b(how|help|guide|steps|where|can i|what is|what are|explain)\b|كيف|شلون|شو الطريقة|كيفيه|وين|ساعدني|مساعده|ما هو|ما هي"#,
        &question,
    ) && !matches(
        r#"\b(expired|expire|lost|forgot|forgotten|reset|missing|problem|trouble|not working|doesnt work|does not work)\b|انته|ضاعت|نسيت|مشكله|ما عم|ما بقدر|ما وصل|ما بيشتغل"#,
        &question,
    ) {
        return None;
    }
    if matches(
        r#"\b(password|passcode)\b|كلمه السر|كلمه المرور|باسورد"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Open /menu → My account → Profile to change your password on the Wartaq website. If you forgot it, ask an administrator for a reset. Never send your password here."#,
            r#"افتح /menu ثم حسابي ثم الملف الشخصي لتغيير كلمة المرور على موقع وارتق. إذا نسيتها، اطلب من المسؤول إعادة تعيينها. لا ترسل كلمة المرور هنا."#,
        ));
    }
    if matches(
        r#"\b(unlink|disconnect|unpair)\b|الغاء الربط|الغي الربط|فك الربط|افصل الحساب"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Send /unlink to disconnect this chat from Wartaq. Messages already delivered to Telegram may remain visible."#,
            r#"أرسل /unlink لإلغاء ربط هذه المحادثة بوارتق. قد تبقى الرسائل التي وصلت إلى تيليغرام ظاهرة."#,
        ));
    }
    if matches(
        r#"\b(language|arabic|english)\b|اللغ[ةه]|العربي[ةه]|الانجليزي[ةه]|عربي|انكليزي"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Send /language or use the language button to switch between Arabic and English."#,
            r#"أرسل /language أو اضغط زر اللغة للتبديل بين العربية والإنجليزية."#,
        ));
    }
    if matches(r#"\b(link|connect|linking)\b|ربط|اربط"#, &question) {
        return Some(lang.text(
            r#"Send /linkaccount, open Wartaq, sign in and approve the connection. Copy the eight-digit code and send it in this same private chat within five minutes. Administrators use this browser route."#,
            r#"أرسل /linkaccount وافتح وارتق، ثم سجّل الدخول ووافق على الربط. انسخ الرمز المؤلف من ثمانية أرقام وأرسله في نفس المحادثة الخاصة خلال خمس دقائق. المسؤول يستخدم الربط عبر المتصفح."#,
        ));
    }
    if matches(
        r#"\b(phone|number|sign in|signin|log in|login|start|account)\b|هاتف|رقم|تسجيل الدخول|سجل دخول|ادخل|حساب"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Send /sharephone and use Telegram’s Share my phone button. A student with one matching active roster record can open their own attendance; teachers can use their verified sign-in number. If it does not work, ask your teacher to check your number, or use /linkaccount."#,
            r#"أرسل /sharephone واضغط زر مشاركة رقم هاتفي في تيليغرام. الطالب الذي يطابق رقمه سجلاً نشطاً واحداً يمكنه فتح حضوره، والمعلم يستخدم رقم دخوله المؤكد. إذا ما زبطت، اطلب من معلمك التأكد من الرقم أو استخدم /linkaccount."#,
        ));
    }
    if matches(
        r#"\b(privacy|private|secure|data|questions|messages)\b|خصوصي|سري|بيانات|اسئلتي|رسائلي"#,
        &question,
    ) {
        return Some(lang.text(
            r#"The assistant uses the text of some questions to choose where to look. Leave out passwords, codes and phone numbers."#,
            r#"يستخدم المساعد نص بعض الأسئلة ليحدد أين يبحث. لا ترسل كلمات المرور أو الرموز أو أرقام الهاتف."#,
        ));
    }
    if matches(
        r#"\b(offline|internet|browser|website)\b|بدون نت|بدون انترنت|اوفلاين|متصفح|الموقع"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Offline attendance and device settings are available on the Wartaq website in your browser. Reconnect to sync pending changes."#,
            r#"الحضور دون إنترنت وإعدادات الجهاز متاحة على موقع وارتق في المتصفح. أعد الاتصال لمزامنة التغييرات المعلّقة."#,
        ));
    }
    if matches(
        r#"\b(export|download|excel|report|reports)\b|تصدير|صدر|تحميل|نزل|اكسل|تقارير|تقرير"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Administrators can open /menu → Courses, choose a course and download its Excel reports. The available reports include grades, students and attendance."#,
            r#"يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وتنزيل تقارير Excel، ومنها الدرجات والطلاب والحضور."#,
        ));
    }
    if matches(
        r#"\b(enroll|enrol|enrollment|enrolment|membership|members)\b|تسجيل الطلاب|تسجيل طالب|انتساب|عضويه|ضم طالب"#,
        &question,
    ) {
        return Some(lang.text(
            r#"An administrator can open /menu → Courses, choose a course and manage its members there. Course levels are captured from the current student level when first enrolled."#,
            r#"يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وإدارة الطلاب المنتسبين. يُحفظ مستوى الطالب الحالي في الدورة عند تسجيله أول مرة."#,
        ));
    }
    if matches(
        r#"\b(grade|grades|score|scores|exam|marks)\b|علامه|علامات|درجه|درجات|امتحان"#,
        &question,
    ) {
        return Some(lang.text(
            r#"An administrator can open /menu → Courses, choose a course and record grades for its students there."#,
            r#"يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وتسجيل علامات طلابها هناك."#,
        ));
    }
    if matches(
        r#"\b(note|notes|correction|correct|mistake|history)\b|ملاحظه|ملاحظات|تصحيح|صحح|خطا|السجل"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Open /menu → Students, find the student and open their profile. There you can add a tagged note or review history and correct an entry."#,
            r#"افتح /menu ثم الطلاب، وابحث عن الطالب وافتح ملفه. هناك يمكنك إضافة ملاحظة مع وسوم، أو مراجعة السجل وتصحيح إدخال."#,
        ));
    }
    if matches(
        r#"\b(attendance|present|absent|absence)\b|حضور|غياب|حاضر|غائب"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Open /menu → Take attendance, choose a course and date, then mark each student Present, Absent or Excused. The refreshed roll shows what was saved. Students can open My attendance to see their own record."#,
            r#"افتح /menu ثم تسجيل الحضور، واختر الدورة والتاريخ، ثم حدّد لكل طالب حاضر أو غائب أو معذور. تظهر العلامة المحفوظة في القائمة المحدّثة. الطالب يستطيع فتح حضوري لرؤية سجله."#,
        ));
    }
    if matches(
        r#"\b(menu|help|wartaq)\b|القائم[ةه]|وارتق|مساعده"#,
        &question,
    ) {
        return Some(lang.text(
            r#"Open /menu to choose a task. Ask me how to sign in, take attendance, use courses or find a feature. Use /cancel to leave an active workflow first."#,
            r#"افتح /menu واختر المهمة. فيك تسألني كيف تسجّل الدخول أو تسجّل الحضور أو تستخدم الدورات. استخدم /cancel للخروج من أي خطوة مفتوحة أولاً."#,
        ));
    }
    None
}
