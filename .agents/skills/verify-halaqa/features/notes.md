# Tagged notes

## Sub-features

Student notes from attendance rows, saved notes on the student's profile, Arabic text and optional normalized comma-separated tags.

## How to get to it (user POV)

Open **Attendance**, choose the **Session date**, and use the amber **Add note** icon beside the status buttons on a student's row. Read the selected student and session date in the dialog. Enter the note and choose **Save note**. Blank tags use the default attendance tag. Open the student's profile from **Students** or **Attendance** to read their dated notes in **Student notes**. Administrators can review original note events in **Profile → Settings → History & corrections**.

## Driving it with Playwright

The synthetic authenticated notebook scenario opens the dialog for ليلى, tests Escape and focus return, and saves Arabic text and tags independently of attendance. Verify note text stays off the attendance page, its Add note dialog, and the Students list. Open student profiles to check isolation, dates, tags and the effect of a correction after reload. The scenario writes `.artifacts/verification/notebook.png`. A separate scenario verifies a legacy Notes bookmark redirects to Attendance, then checks the date and student on the saved event and reads the note on the profile after reload. The phone scenario saves and opens a note in Arabic at 320px and checks notebook layouts at 390px and in landscape.

## Gotchas

Notes remain student-scoped across courses. React renders the note as text and the API validates the payload. Tags normalize to lowercase, blank notes are rejected, and corrections hide the effective note while preserving its original History record. Legacy Notes bookmarks open Attendance.
