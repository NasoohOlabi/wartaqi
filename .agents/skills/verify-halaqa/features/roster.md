# Student roster

## Sub-features

Administrators manage assignments in Settings. Teachers see their assigned roster.

In the notebook **Students** list, verify distinct Student, Level, Active and Actions columns rather than concatenated name/level text. With no course selected, Level shows the roster group value. With a course selected, it shows only the matching enrollment level; a blank enrollment shows an em dash. Use one synthetic student with different roster, Course A and Course B levels, switch courses and reload, and verify the exact Level cell and Open student action. Preserve distinctly labelled fields with no horizontal scrolling at 320/390/430px in English and Arabic.

On compact screens, the first record appears immediately after the roster toolbar. In All students, Sort and Filter sit beside the heading and have all seven fields. At 320px with a course selected, verify the toolbar wraps as needed and keeps the heading and Export contacts readable. Each student's full name opens Student information directly. Verify the status icon sits beside the name and the labelled phone, attendance, level and courses follow below. Check that Back returns to the roster. Verify sorting and filtering from the toolbar, reload and clearing. Check the wide table still has sortable headers and separate name columns.

Open **Add student** from the dashboard and Administrator **Settings → Students**. Verify the creation form has no student table or table controls below it in English and Arabic, on desktop and mobile. Save or cancel and confirm the table returns with separate **First name**, **Last name**, **Level**, **WhatsApp**, **Location**, **Birth year**, **Teacher**, **Active** and **Actions** columns. Create and edit a synthetic student with separate given/family names, save, and reopen Edit to confirm each field persisted independently. Include a compound Arabic given/family name, a mononym with blank family name, active/inactive students and a legacy unsplit name. Level uses the roster group value. Legacy names remain intact until edited.

In **Settings → Students**, verify the person-plus icon shares the heading toolbar with Sort and Filter. At 320/390/430px, verify the pencil sits beside the name fields and WhatsApp shares the number line. Both icons open their forms directly, have localized accessible labels and tooltips, and retain 44px tap targets. Desktop keeps the Actions column.

As an Administrator, enter Students and verify the saved default course is selected; without a default, **All students** is selected. Choose **All students** to inspect the full register. Active, inactive assigned and inactive unassigned students appear with the full register count. Search/group filters narrow the table without altering stored active state. An explicit course filter survives reload; clearing it restores the full list. Current attendance, dashboard and Grades retain their prior active-only scope, while historical course summaries retain inactive memberships; Teacher responses remain restricted to active assigned students. Attempt a new note/attendance event for an inactive student directly against the server and verify denial with no appended rows; exact event retries for eligible students remain accepted.

Verify a distinct labelled **Courses** column with localized None for zero active memberships and all four named badges for a four-course student, including a closed course but excluding a withdrawn membership. Keep course names separate from roster/enrollment levels. Check course-filter switching, reload, desktop/landscape use of space and an expandable course list at 320/390px in English/Arabic; every course name must remain reachable without page-level overflow. Account actions use the existing profile menu and `e2e/profileActions.ts` helpers.

## How to get to it (user POV)

Verify the primary Students columns **First name**, **Last name**, **Phone**, **Attendance** before Level/Courses/status/actions. Use explicit compound names, a leading-zero phone, a missing phone and an inactive historical student. Open student must show Student information with those values, status totals, effective recorded-session percentage and history. Take attendance is a separate eligible action. All students aggregates attendance across accessible courses and unscoped marks, while a course filter narrows it. Include corrections, legacy statuses and genuinely unmarked dates. Reload, Back, unknown/foreign student IDs and English/Arabic 320/390px paths must preserve profile privacy, labelled fields, LTR phones and no horizontal table scrolling. Student responses must not include staff contact/name-part fields.

The Student portal also keeps Student and Roster level in separate labelled fields; it must not imply that the roster level is every course's level. Account-to-student selectors show only the student's name. Student snapshot history exposes Level and Active as distinct fields, retaining the raw record under Record details. Check these in `e2e/roles.spec.ts` and `e2e/dashboard.spec.ts` alongside attendance so a fix cannot leave another rendering path unchanged.

## Driving it with Playwright

On desktop and an emulated Arabic phone, search `ابرا` for a synthetic student with the family name `إبراهيم`. Verify the partial name and a typo both find the student, an unrelated query shows the empty state, and clearing restores the list. Check the search combines with group and course scope, and that free-text column filters handle typos while phone lookups stay literal.

Run `e2e/roles.spec.ts` for Administrator student creation and editing. Run `e2e/notebook.spec.ts` to verify Arabic roster search and attendance and notes for enrolled students.

## Gotchas

Use a row-scoped locator because editable names/groups repeat. Renaming must preserve attendance/note references. Deactivation does not delete history.

The Administrator register stacks distinctly labelled fields at phone widths. Verify English and Arabic labels, row-scoped Edit, and no horizontal table or page overflow at 320/390/430px. Keep synthetic table screenshots under ignored `.artifacts/verification/`. Apply ordered migrations through `neon/007-student-profiles-handbooks.sql` before testing the updated administrator API against a disposable database; local/synthetic proof does not establish deployed migration status.

Verify optional WhatsApp, location and birth year fields on create and edit, including blank clearing, reload and distinct labelled roster columns at 320/390px in both languages. Birth year is a year only.

Check notebook Students and grouped/searched attendance column geometry on desktop in both languages: Level stays compact, Student receives the remaining width, and level text is centered under its header even when the text direction differs. Check labelled mobile fields, direct actions and readable longer course levels without horizontal scrolling. `expectCompactLevel` in `e2e/courses.spec.ts` measures actual rendered widths and text alignment.

The notebook Students status uses a circled check for active students and a circled minus for inactive students. Verify localized accessible image names and tooltips in English and Arabic. Desktop icons are centered in the status column. On compact screens check that the icon shares the name row and there is no standalone status row.

Verify an active and an inactive unassigned student appear in the full administrator list with localized Unassigned, edit and save without an assignment, then assign an approved active teacher. Existing inactive or provisional teacher assignments remain selectable for profile edits but cannot be newly assigned. Verify teachers cannot access unassigned students. Apply `neon/009-unassigned-students.sql` to disposable databases.

Staff phone displays in Students, Student information and Administrator register use isolated LTR dial links. Use synthetic Syrian mobile numbers with unspaced and mixed-spacing inputs. Check `09XX XXX XXX` and `+963 9XX XXX XXX` in English and Arabic on desktop and phones. Verify leading zeros, bare international and explicit +/00 dial targets, missing numbers and invalid values. Edits, filters and sorting use the stored values.

In **Students**, select a course and verify the **WhatsApp** icon opens the exact `wa.me` target for an unsaved synthetic number. Check the localized accessible label, 44px tap target and separate dial link. Verify the icon sits alongside the phone number in English and Arabic at 320/360/390/430px. **Export contacts** is a 44px download icon with a localized accessible name and tooltip. Check that Export, Sort and Filter share a row and that the Students heading remains readable at the same widths and on desktop. Use the export icon with a search or table filter applied and inspect the `.vcf` for the entire accessible active course roster with usable phone numbers, including a UTF-8 Arabic name. Check skipped-number feedback, inactive and withdrawn memberships, another course, and the absence of the export in All students. The browser test intercepts the synthetic WhatsApp destination. Native contact imports and WhatsApp handoff require a physical device.

Open a student at 1624px, 1920px and 2560px in English and Arabic. Profile fields and courses align above the attendance and notes sections. Attendance totals sit above the history table and notes sit alongside them. Confirm the first record is visible on a 560px-high viewport, the history date sort works and Back and Edit student remain reachable. Check the empty states. At 320/390/430px, check that Add note is reachable before the history rows and that the labelled records fit without horizontal scrolling. `e2e/table-pages.spec.ts` exercises both layouts with synthetic records.

At 894px and wider, check that **Edit student** appears beside the profile heading with a pencil icon. Back has a direction-aware arrow. Take attendance and Add note have distinct icons beside their section headings. Check English and Arabic, including a 320px phone where the header and action buttons can wrap. Keep the localized labels and 44px button targets. The administrator editor remains a separate page, while the Teacher editor opens within Student information.

Whole-number decimal levels display without the zero-only fraction in roster, filters, profile, history and student portal. Editing a different field preserves the original stored level. Fractional and named levels remain unchanged.
