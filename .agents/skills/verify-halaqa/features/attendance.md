# Attendance and summary

## Sub-features

Present, Absent and Excused save on tap. Tap the selected status again to clear that student's mark for the current course and date. Attendance includes a session date, an unmarked filter and course-wide summary across all dates. Historical early/late marks display and count as Present while their raw events remain unchanged.

## How to get to it (user POV)

Sign in to open the dashboard. Use **Take attendance** and choose an open course on Attendance to mark its enrolled students in name order. Resume a partly marked session on the same course and date. Toggle Show unmarked and verify saved marks survive filters and student lookups. The dashboard lists the current students with the most explicit absences in the last six recorded sessions through the selected date. Open a name to review the student. Reversing an absence changes the outreach list. **Summary** shows all-date totals for the selected course's authorized roster, including inactive memberships, with per-column table filters. Notes remain student-scoped and count across all dates.

## Driving it with Playwright

Open Attendance without a course and follow the course selection action. In Courses, choose an open course as current, then open Attendance and read the alphabetical roster without typing or expanding a group. Check the enrollment-specific Teacher and Level cells and mark a student by name. Check both Arabic and English on desktop and at 320/390/430px. A closed course permits historical inspection only.

On Dashboard, check that each follow-up card fits its localized weekday and numeric day/month on one line. All six cards share a row when their labels fit and wrap on narrower phones. The top border is red for Absent, green for Present, amber for Excused and dashed for Unmarked. Screen readers still announce the localized status. `e2e/dashboard.spec.ts` checks the colors, weekday, date order and overflow in English and Arabic at 320/360/390/430/662/1280px.

At phone widths, verify Sort and Filter beside the Attendance title. Each record starts with its student name above the labelled level, teacher and attendance fields. The attendance field has three 44px segments in one pill-shaped track, with a transparent, borderless amber Add note button on its right in English and Arabic. Confirm the layout at 320/390/430px and on desktop. The same toolbar sits beside the Summary title. Desktop tables show sortable column headings.

The authenticated notebook test opens a course with the synthetic student ليلى, presses **Absent** in the **Attendance for ليلى** group and asserts the matching Absent tally on Summary. Verify the selected tile has a green Present, red Absent or amber Excused tint in both themes and languages. Arrow keys move focus in the page's reading direction, and Space toggles the focused status. An unmarked row has zero pressed buttons. Repeat the selected status with touch or keyboard and verify all buttons are unpressed, including after reload. Check that the Save and bulk marking actions are absent. Use **Correct this mark** to reverse a saved mark when permitted. Confirm legacy early/late events contribute to Present.

Vary **Session date** and combine per-column filters, including a no-match query. Mark one visible student and verify other students and dates keep their recorded status. With **Show unmarked** on, confirm the row disappears after the local save. Check that rapid taps are applied in order when sync is delayed.

For Summary, use historical and current attendance in two courses. Assert course switching and reload retain independent all-date totals and inactive membership history. Saved `from`, `to`, `date`, `q`, `group`, `student` and `all` parameters must not narrow Summary. The page-level date, student-search and group controls are absent; per-column table filters narrow its rows without changing the overview totals. Other tabs retain their page controls. Check desktop/mobile in English/Arabic. Previous / no course counts only unscoped attendance.

In `e2e/summary.spec.ts`, inspect the dates ranked by recorded absences and expand one to check names and marks. Verify the remaining dates, historical memberships, course switching, no records and zero absences. At 1920px, confirm the student totals sit beside the session bars with the columns mirrored in Arabic. On a phone, confirm the sections stack in reading order without horizontal overflow. Check the labelled bars and the exact counts in the student table.

Open **Export attendance chart** on Summary with a Teacher and a multi-page course. Verify that the sorted percentages and fractions reflect corrected marks in this course, students with no records appear last, and classmates keep anonymous labels when one student is highlighted. Check the downloaded PNG against the preview, the named export opt-in, navigation to the highlighted student's page, and privacy defaults after closing. Open the ZIP and check all page images. Exercise file sharing, cancellation, and download on a browser without file sharing. Check English and Arabic at phone and desktop widths. The simulated share target in `e2e/summary.spec.ts` verifies the file passed to the browser. Physical-device delivery is a separate check.

## Gotchas

Same student/date uses last surviving event in database sequence, never device clock. A clear makes the session unmarked and excludes it from totals. Reversing the newest event in history reveals the prior surviving status. Summary ignores saved date ranges, including reversed bounds.

Keep a legacy raw mark in projection/API tests, reverse a newer mark to reveal it as Present, and verify the original status remains in the raw event. An identical legacy retry stays idempotent; reusing its ID for a different raw status still conflicts even if both statuses project to Present. Course enrollment scopes remain independent.

## Offline attendance

Check that the dashboard and Attendance page have no offline shortcut in their content and **My changes** is absent from Profile with an empty queue. Disconnect and verify the offline notebook opens automatically. After reconnecting with pending changes, find **My changes** in Profile in English and Arabic. The review page keeps the desktop sidebar or mobile header and bottom navigation. Use the navigation to leave, reopen the review, and resolve the remaining marks and phone edits. After sync, confirm the compact completion message and the menu item's disappearance without a reload.

Open the notebook once online as an approved Teacher. Reopen Wartaq offline after its online session would have expired and inspect every assigned student. Pick an open course and date, mark students, add two notes and change the phone for an unenrolled student. Reload and verify all five changes plus the full roster remain available. Reconnect with competing attendance and phone values, a lost successful reply, and a temporarily rejected mark. Verify that notes remain independent, retries preserve IDs, **Sync now** retries the blocked mark after eligibility returns, and an authorized refresh prunes a student reassigned away. Exercise the scenario on desktop and a touch-emulated phone with `e2e/offline-attendance.spec.ts`.
