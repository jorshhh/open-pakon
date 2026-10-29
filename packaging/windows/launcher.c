/*
 * Pakon.exe: starts the app (packaging/macos/pakon_app.py) with the embedded
 * Python next to it, so the window, the taskbar and Task Manager show Pakon
 * and its icon rather than pythonw's. No console (-mwindows).
 */
#include <windows.h>
#include <wchar.h>

typedef int (*py_main_fn)(int argc, wchar_t **argv);

static int fail(const wchar_t *what)
{
    MessageBoxW(NULL, what, L"Pakon", MB_ICONERROR | MB_OK);
    return 1;
}

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE prev, PWSTR cmd, int show)
{
    (void)inst; (void)prev; (void)cmd; (void)show;
    wchar_t dir[MAX_PATH], dll[MAX_PATH], script[MAX_PATH];
    DWORD n = GetModuleFileNameW(NULL, dir, MAX_PATH);
    if (n == 0 || n >= MAX_PATH)
        return fail(L"Cannot find the install folder.");
    wchar_t *slash = wcsrchr(dir, L'\\');
    if (slash)
        *slash = 0;
    _snwprintf(dll, MAX_PATH, L"%ls\\python312.dll", dir);
    _snwprintf(script, MAX_PATH, L"%ls\\packaging\\macos\\pakon_app.py", dir);

    HMODULE py = LoadLibraryExW(dll, NULL, LOAD_WITH_ALTERED_SEARCH_PATH);
    if (!py)
        return fail(L"python312.dll is missing: reinstall Pakon.");
    py_main_fn py_main = (py_main_fn)(void *)GetProcAddress(py, "Py_Main");
    if (!py_main)
        return fail(L"python312.dll has no Py_Main: reinstall Pakon.");

    /* argv[0] is this exe (sys.executable); -I ignores PYTHON* variables
     * and the user's site-packages; -X utf8 reads and writes text (settings,
     * scripts, the log) as UTF-8, as on macOS and Linux, not the code page */
    int argc;
    wchar_t **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    wchar_t *args[64] = { argv ? argv[0] : L"Pakon.exe", L"-I", L"-X", L"utf8", script };
    int nargs = 5;
    for (int i = 1; argv && i < argc && nargs < 63; i++)
        args[nargs++] = argv[i];
    args[nargs] = NULL;
    return py_main(nargs, args);
}
