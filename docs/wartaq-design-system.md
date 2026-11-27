# Wartaq design system

The public course homepage is separate from the account and notebook entry at `?page=app`. Course teacher, schedule and enrollment details are placeholders. `src/HomePage.tsx` and `src/home.css` own public presentation; `src/App.tsx` loads the notebook on entry. `src/Workspace.tsx` and `src/workspace.css` own the notebook; `src/NeonAuth.tsx` owns account flows. Shared brand assets live in `src/brand/`.

`/?design=a` and `/?design=b` are local sample design routes, not authenticated notebooks. `e2e/home.spec.ts`, `e2e/wartaq.spec.ts`, `e2e/mobile.spec.ts` and `e2e/neon.spec.ts` cover homepage, design drafts, responsive layout and authentication with synthetic data. Physical-device and live-provider verification remain separate.

## Signed-in navigation

- Keep the main destinations in the desktop rail and the phone bottom bar. Highlight the current destination throughout its subpages. Administrators can open Settings directly. Students can switch between My attendance and Profile settings directly. The account menu holds personal and session actions.
- Use the same content frame and inline-start alignment throughout the workspace, including Settings. Use `WorkspacePageHeader` for the page title, a named parent link when there is one, and the page's primary actions. Align actions with the title when space allows. Use the same logical alignment in Arabic.
- On phones, put the named return action for a child screen at the leading edge of the top app bar. Use a 48px icon button with a localized name and tooltip. The arrow points right in Arabic and left in English. Desktop keeps its labelled return in the page. Show the arrow when the child screen has a return destination.
- Place links to sibling pages directly under the header. Course Students, Grades and History share a heading structure and tabs, and keep Courses selected. Settings sections use the same navigation treatment. Course History opens under the selected course. History & corrections under Settings retains its own course selector.
- Put related links where the user needs them. Student names in course records open the student, the student record links back to the course, and the edit form stays under the student. Use a named return that restores the calling view and its URL filters, with a parent destination for direct links.
- Keep all main destinations visible at 320px in English and Arabic. Use at least 44px touch targets, visible keyboard focus and a current-page indicator. Keep the sidebar usable while a long page scrolls.
