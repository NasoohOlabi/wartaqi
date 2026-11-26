type Language = 'en' | 'ar'
type FallbackReason = 'unsupported' | 'unavailable' | 'invalid' | 'denied'

const fallback: Record<FallbackReason, Record<Language, string>> = {
  unsupported: {
    en: 'I can help with Wartaq questions. For other requests, please ask your teacher. Use /menu to open Wartaq.',
    ar: 'فيني ساعدك بأسئلة وارتق. للطلبات الثانية اسأل معلمك، واستخدم /menu لفتح القائمة.',
  },
  unavailable: {
    en: 'Sorry, I cannot answer right now. Please try again, or use /menu and ask your teacher for help.',
    ar: 'عذراً، ما قدرت جاوب هلق. جرّب مرة ثانية، أو افتح /menu واطلب المساعدة من معلمك.',
  },
  invalid: {
    en: 'Sorry, I could not prepare a reliable answer. Please try asking another way or use /menu.',
    ar: 'عذراً، ما قدرت أعطيك جواب موثوق. جرّب تسأل بطريقة ثانية أو افتح /menu.',
  },
  denied: {
    en: 'Open /menu to check that you’re signed in. If you still can’t see the record, ask your teacher or an administrator.',
    ar: 'افتح /menu لتتأكد أنك مسجّل الدخول. إذا ما ظهر السجل، اسأل معلمك أو المسؤول.',
  },
}

export function fallbackReply(lang: Language, reason: FallbackReason): string {
  return fallback[reason][lang]
}

export function cannedReply(question: string, lang: Language): string | undefined {
  const q = question
    .toLowerCase()
    .normalize('NFKC')
    .replace(/[\u064b-\u065f\u0670\u0640]/g, '')
    .replace(/[إأآ]/g, 'ا')
    .replace(/ى/g, 'ي')
    .replace(/[^\p{L}\p{N}\s]/gu, ' ')
    .replace(/\s+/g, ' ')
    .trim()
  const reply = (en: string, ar: string) => (lang === 'ar' ? ar : en)
  const has = (pattern: RegExp) => pattern.test(q)

  if (/^(hi|hello|hey|salam|مرحبا|اهلا|هلا|السلام عليكم|سلام|صباح الخير|مسا الخير)$/.test(q))
    return reply(
      'Hello! Ask me about using Wartaq, or open /menu to get started.',
      'أهلاً! اسألني عن استخدام وارتق، أو افتح /menu لنبدأ.',
    )

  if (has(/\b(privacy|who can (?:see|read) my (?:questions|messages))\b|خصوصي|مين بيشوف اسئلتي/))
    return reply(
      'The assistant uses the text of some questions to choose where to look. Leave out passwords, codes and phone numbers. Students can only see their own attendance.',
      'يستخدم المساعد نص بعض الأسئلة ليحدد أين يبحث. لا ترسل كلمات المرور أو الرموز أو أرقام الهاتف. الطالب يشوف سجل حضوره فقط.',
    )

  if (has(/^(what can (?:you|this bot) do|help(?: me)?|شو بتقدر تعمل|شو فيني اعمل)$/))
    return reply(
      'Ask how to sign in or use Wartaq. If you’re a student, try “Show my attendance”. Staff can list courses or search for a student. Open /menu for more options.',
      'اسأل كيف تسجّل الدخول أو تستخدم وارتق. إذا كنت طالباً، جرّب «أرني سجل حضوري». يمكن للمعلمين والمسؤولين عرض الدورات أو البحث عن طالب. افتح /menu للمزيد.',
    )

  if (has(/\bcancel\b|الغي الخطوه|الغي العمليه|كيف بلغي/))
    return reply(
      'Send /cancel to leave the current step, then /menu to choose another task.',
      'أرسل /cancel للخروج من الخطوة الحالية، ثم /menu لاختيار مهمة ثانية.',
    )

  if (
    has(
      /^(?:please )?(?:mark|record)\b.*\b(?:present|absent|excused)\b|^(?:سجل|حط)\s.*(?:حاضر|غائب|غايب|معذور)/,
    )
  )
    return reply(
      'Open /menu → Take attendance, choose the course and date, then tap the mark next to the student’s name. The list shows the saved mark.',
      'افتح /menu ثم تسجيل الحضور. اختر الدورة والتاريخ، واضغط الحالة بجانب اسم الطالب. تظهر العلامة المحفوظة في القائمة.',
    )

  // Keep requests for current records on the live-data path.
  if (
    has(
      /\b(show|list|who|what is my|what are my|give me)\b|\u0645\u064a\u0646|\u0645\u0646 \u063a\u0627\u0628|\u0645\u0646 \u062d\u0636\u0631|\u0627\u0639\u0631\u0636|\u0627\u0631\u064a\u0646\u064a|\u0634\u0648 \u062d\u0636\u0648\u0631\u064a/,
    )
  )
    return undefined

  const how = has(
    /\b(how|help|guide|steps|where|can i|what is|what are|explain)\b|كيف|شلون|شو الطريقة|كيفيه|وين|ساعدني|مساعده|ما هو|ما هي/,
  )
  const trouble = has(
    /\b(expired|expire|lost|forgot|forgotten|reset|missing|problem|trouble|not working|doesnt work|does not work)\b|انته|ضاعت|نسيت|مشكله|ما عم|ما بقدر|ما وصل|ما بيشتغل/,
  )

  if (has(/\b(code|codes)\b|كود|رمز/) && has(/\b(expired|expire|expires)\b|انته|خلص|صلاحي/))
    return reply(
      'A link code lasts five minutes. Send /cancel then /linkaccount. Sign in and approve again, then send the new eight-digit code by itself in this same private chat.',
      'رمز الربط صالح لخمس دقائق. أرسل /cancel ثم /linkaccount. سجّل الدخول ووافق من جديد، ثم أرسل الرمز الجديد من ثمانية أرقام وحده في نفس المحادثة الخاصة.',
    )
  if (has(/\bwhat does excused mean\b|شو يعني معذور|ما معني معذور/))
    return reply(
      'Excused means the teacher marked the absence as excused. Wartaq counts it separately from Present and Absent.',
      'معذور يعني أن المعلم سجّل غيابك بعذر. وارتق يحسبه بشكل منفصل عن حاضر وغائب.',
    )
  if (!how && !trouble) return undefined

  if (has(/\b(password|passcode)\b|كلمه السر|كلمه المرور|باسورد/))
    return reply(
      'Open /menu → My account → Profile to change your password on the Wartaq website. If you forgot it, ask an administrator for a reset. Never send your password here.',
      'افتح /menu ثم حسابي ثم الملف الشخصي لتغيير كلمة المرور على موقع وارتق. إذا نسيتها، اطلب من المسؤول إعادة تعيينها. لا ترسل كلمة المرور هنا.',
    )
  if (has(/\b(unlink|disconnect|unpair)\b|الغاء الربط|الغي الربط|فك الربط|افصل الحساب/))
    return reply(
      'Send /unlink to disconnect this chat from Wartaq. Messages already delivered to Telegram may remain visible.',
      'أرسل /unlink لإلغاء ربط هذه المحادثة بوارتق. قد تبقى الرسائل التي وصلت إلى تيليغرام ظاهرة.',
    )
  if (has(/\b(language|arabic|english)\b|اللغ[ةه]|العربي[ةه]|الانجليزي[ةه]|عربي|انكليزي/))
    return reply(
      'Send /language or use the language button to switch between Arabic and English.',
      'أرسل /language أو اضغط زر اللغة للتبديل بين العربية والإنجليزية.',
    )
  if (has(/\b(link|connect|linking)\b|ربط|اربط/))
    return reply(
      'Send /linkaccount, open Wartaq, sign in and approve the connection. Copy the eight-digit code and send it in this same private chat within five minutes. Administrators use this browser route.',
      'أرسل /linkaccount وافتح وارتق، ثم سجّل الدخول ووافق على الربط. انسخ الرمز المؤلف من ثمانية أرقام وأرسله في نفس المحادثة الخاصة خلال خمس دقائق. المسؤول يستخدم الربط عبر المتصفح.',
    )
  if (
    has(
      /\b(phone|number|sign in|signin|log in|login|start|account)\b|هاتف|رقم|تسجيل الدخول|سجل دخول|ادخل|حساب/,
    )
  )
    return reply(
      'Send /sharephone and use Telegram’s Share my phone button. A student with one matching active roster record can open their own attendance; teachers can use their verified sign-in number. If it does not work, ask your teacher to check your number, or use /linkaccount.',
      'أرسل /sharephone واضغط زر مشاركة رقم هاتفي في تيليغرام. الطالب الذي يطابق رقمه سجلاً نشطاً واحداً يمكنه فتح حضوره، والمعلم يستخدم رقم دخوله المؤكد. إذا ما زبطت، اطلب من معلمك التأكد من الرقم أو استخدم /linkaccount.',
    )
  if (has(/\b(privacy|private|secure|data|questions|messages)\b|خصوصي|سري|بيانات|اسئلتي|رسائلي/))
    return reply(
      'The assistant uses the text of some questions to choose where to look. Leave out passwords, codes and phone numbers.',
      'يستخدم المساعد نص بعض الأسئلة ليحدد أين يبحث. لا ترسل كلمات المرور أو الرموز أو أرقام الهاتف.',
    )
  if (has(/\b(offline|internet|browser|website)\b|بدون نت|بدون انترنت|اوفلاين|متصفح|الموقع/))
    return reply(
      'Offline attendance and device settings are available on the Wartaq website in your browser. Reconnect to sync pending changes.',
      'الحضور دون إنترنت وإعدادات الجهاز متاحة على موقع وارتق في المتصفح. أعد الاتصال لمزامنة التغييرات المعلّقة.',
    )
  if (has(/\b(export|download|excel|report|reports)\b|تصدير|صدر|تحميل|نزل|اكسل|تقارير|تقرير/))
    return reply(
      'Administrators can open /menu → Courses, choose a course and download its Excel reports. The available reports include grades, students and attendance.',
      'يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وتنزيل تقارير Excel، ومنها الدرجات والطلاب والحضور.',
    )
  if (
    has(
      /\b(enroll|enrol|enrollment|enrolment|membership|members)\b|تسجيل الطلاب|تسجيل طالب|انتساب|عضويه|ضم طالب/,
    )
  )
    return reply(
      'An administrator can open /menu → Courses, choose a course and manage its members there. Course levels are captured from the current student level when first enrolled.',
      'يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وإدارة الطلاب المنتسبين. يُحفظ مستوى الطالب الحالي في الدورة عند تسجيله أول مرة.',
    )
  if (has(/\b(grade|grades|score|scores|exam|marks)\b|علامه|علامات|درجه|درجات|امتحان/))
    return reply(
      'An administrator can open /menu → Courses, choose a course and record grades for its students there.',
      'يمكن للمسؤول فتح /menu ثم الدورات واختيار الدورة وتسجيل علامات طلابها هناك.',
    )
  if (has(/\b(note|notes|correction|correct|mistake|history)\b|ملاحظه|ملاحظات|تصحيح|صحح|خطا|السجل/))
    return reply(
      'Open /menu → Students, find the student and open their profile. There you can add a tagged note or review history and correct an entry.',
      'افتح /menu ثم الطلاب، وابحث عن الطالب وافتح ملفه. هناك يمكنك إضافة ملاحظة مع وسوم، أو مراجعة السجل وتصحيح إدخال.',
    )
  if (has(/\b(attendance|present|absent|absence)\b|حضور|غياب|حاضر|غائب/))
    return reply(
      'Open /menu → Take attendance, choose a course and date, then mark each student Present, Absent or Excused. The refreshed roll shows what was saved. Students can open My attendance to see their own record.',
      'افتح /menu ثم تسجيل الحضور، واختر الدورة والتاريخ، ثم حدّد لكل طالب حاضر أو غائب أو معذور. تظهر العلامة المحفوظة في القائمة المحدّثة. الطالب يستطيع فتح حضوري لرؤية سجله.',
    )
  if (has(/\b(menu|help|wartaq)\b|القائم[ةه]|وارتق|مساعده/))
    return reply(
      'Open /menu to choose a task. Ask me how to sign in, take attendance, use courses or find a feature. Use /cancel to leave an active workflow first.',
      'افتح /menu واختر المهمة. فيك تسألني كيف تسجّل الدخول أو تسجّل الحضور أو تستخدم الدورات. استخدم /cancel للخروج من أي خطوة مفتوحة أولاً.',
    )
  return undefined
}
