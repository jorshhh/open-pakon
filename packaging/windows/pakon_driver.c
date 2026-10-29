/*
 * pakon_driver.exe: sets up Windows' own WinUSB driver for the scanner
 * (pakon-winusb.inf next to it), so libusb reaches it without Zadig. The
 * installer runs it with administrator rights:
 *
 *   pakon_driver.exe install [--quiet]     sign the INF for this PC, install it
 *   pakon_driver.exe uninstall [--quiet]   remove the driver package and the
 *                                          certificate
 *
 * Windows installs only signed driver packages. With no certificate of ours
 * that Windows trusts, this makes one for this PC alone, as Zadig does: a
 * self-signed code-signing certificate, a catalog (pakon-winusb.cat) with the
 * INF's hash signed with it, the certificate added to the machine's Root and
 * TrustedPublisher stores, and then its private key deleted, so it cannot
 * sign anything else. The package holds only the INF: the driver itself is
 * winusb.sys, part of Windows. Result in a message box (none on success with
 * --quiet) and the exit code (0 = done); a successful install also leaves
 * MARKER next to it, which tells the installer's updates to skip this step.
 */
#include <windows.h>
#include <wincrypt.h>
#include <wintrust.h>
#include <mscat.h>
#include <mssip.h>
#include <setupapi.h>
#include <shellapi.h>
#include <wchar.h>

#define CERT_CN    L"Pakon USB driver (this computer only)"
#define CONTAINER  L"Pakon USB driver signing"
#define INF_NAME   L"pakon-winusb.inf"
#define CAT_NAME   L"pakon-winusb.cat"
#define MARKER     L"installed-1"        /* installer.nsi; bump with the INF */
#define ZADIG_HELP L"https://github.com/jorshhh/open-pakon/blob/main/packaging/windows/README.md#the-usb-driver"

static const wchar_t *const HWIDS[] = {           /* lower case, as Inf2Cat */
    L"usb\\vid_0f05&pid_f235",    /* before the firmware load */
    L"usb\\vid_0f05&pid_f135",    /* after it */
};

/* mssign32.dll's SignerSignEx; MinGW has no header for it (Microsoft's docs) */
typedef struct { DWORD cbSize; LPCWSTR pwszFileName; HANDLE hFile; } SIGNER_FILE_INFO;
typedef struct {
    DWORD cbSize; DWORD *pdwIndex; DWORD dwSubjectChoice;
    union { SIGNER_FILE_INFO *pSignerFileInfo; void *pSignerBlobInfo; };
} SIGNER_SUBJECT_INFO;
typedef struct {
    DWORD cbSize; PCCERT_CONTEXT pSigningCert; DWORD dwCertPolicy; HCERTSTORE hCertStore;
} SIGNER_CERT_STORE_INFO;
typedef struct {
    DWORD cbSize; DWORD dwCertChoice;
    union { LPCWSTR pwszSpcFile; SIGNER_CERT_STORE_INFO *pCertStoreInfo; void *pSpcChainInfo; };
    HWND hwnd;
} SIGNER_CERT;
typedef struct {
    DWORD cbSize; ALG_ID algidHash; DWORD dwAttrChoice; void *pAttrAuthcode;
    PCRYPT_ATTRIBUTES psAuthenticated; PCRYPT_ATTRIBUTES psUnauthenticated;
} SIGNER_SIGNATURE_INFO;
typedef struct { DWORD cbSize; DWORD cbBlob; BYTE *pbBlob; } SIGNER_CONTEXT;
#define SIGNER_SUBJECT_FILE      1
#define SIGNER_CERT_STORE        2
#define SIGNER_CERT_POLICY_CHAIN 2
#define SIGNER_NO_ATTR           0
typedef HRESULT (WINAPI *SignerSignEx_fn)(DWORD, SIGNER_SUBJECT_INFO *, SIGNER_CERT *,
    SIGNER_SIGNATURE_INFO *, void *, LPCWSTR, PCRYPT_ATTRIBUTES, void *, SIGNER_CONTEXT **);
typedef HRESULT (WINAPI *SignerFreeSignerContext_fn)(SIGNER_CONTEXT *);
/* newdev.dll */
#define DIIRFLAG_FORCE_INF 0x00000002
typedef BOOL (WINAPI *DiInstallDriverW_fn)(HWND, LPCWSTR, DWORD, PBOOL);
typedef BOOL (WINAPI *DiUninstallDriverW_fn)(HWND, LPCWSTR, DWORD, PBOOL);

static int quiet;
static wchar_t dir[MAX_PATH], inf_path[MAX_PATH], cat_path[MAX_PATH], marker[MAX_PATH];

static int report(int ok, const wchar_t *what, DWORD err)
{
    wchar_t msg[1024], sys[512] = L"";
    if (ok) {
        if (!quiet)
            MessageBoxW(NULL, what, L"Pakon USB driver", MB_ICONINFORMATION | MB_OK);
        return 0;
    }
    FormatMessageW(FORMAT_MESSAGE_FROM_SYSTEM | FORMAT_MESSAGE_IGNORE_INSERTS, NULL, err,
                   0, sys, ARRAYSIZE(sys), NULL);
    _snwprintf(msg, ARRAYSIZE(msg),
               L"%ls\n\n%ls(error 0x%08lX)\n\nThe driver can be set up with Zadig "
               L"instead:\n%ls", what, sys, (unsigned long)err, ZADIG_HELP);
    MessageBoxW(NULL, msg, L"Pakon USB driver", MB_ICONERROR | MB_OK);
    return 1;
}

static BOOL is_admin(void)
{
    SID_IDENTIFIER_AUTHORITY nt = SECURITY_NT_AUTHORITY;
    PSID admins;
    BOOL member = FALSE;
    if (AllocateAndInitializeSid(&nt, 2, SECURITY_BUILTIN_DOMAIN_RID,
                                 DOMAIN_ALIAS_RID_ADMINS, 0, 0, 0, 0, 0, 0, &admins)) {
        CheckTokenMembership(NULL, admins, &member);
        FreeSid(admins);
    }
    return member;
}

/* Our certificates in the machine's Root and TrustedPublisher stores: removed
 * (add == NULL), or `add` put there (its encoding only, never the key). */
static BOOL machine_stores(PCCERT_CONTEXT add)
{
    static const wchar_t *const stores[] = { L"Root", L"TrustedPublisher" };
    BOOL ok = TRUE;
    for (int i = 0; i < 2; i++) {
        HCERTSTORE st = CertOpenStore(CERT_STORE_PROV_SYSTEM_W, 0, 0,
                                      CERT_SYSTEM_STORE_LOCAL_MACHINE, stores[i]);
        if (!st)
            return FALSE;
        PCCERT_CONTEXT c = NULL;
        while ((c = CertFindCertificateInStore(st, X509_ASN_ENCODING, 0,
                                               CERT_FIND_SUBJECT_STR_W, CERT_CN, c))) {
            CertDeleteCertificateFromStore(CertDuplicateCertificateContext(c));
        }
        if (add)
            ok &= CertAddEncodedCertificateToStore(st, X509_ASN_ENCODING, add->pbCertEncoded,
                                                   add->cbCertEncoded,
                                                   CERT_STORE_ADD_REPLACE_EXISTING, NULL);
        CertCloseStore(st, 0);
    }
    return ok;
}

static void delete_key(void)
{
    HCRYPTPROV p;
    CryptAcquireContextW(&p, CONTAINER, MS_ENH_RSA_AES_PROV_W, PROV_RSA_AES,
                         CRYPT_MACHINE_KEYSET | CRYPT_DELETEKEYSET | CRYPT_SILENT);
}

/* A self-signed code-signing certificate, its key in CONTAINER. */
static PCCERT_CONTEXT make_cert(void)
{
    HCRYPTPROV prov;
    HCRYPTKEY key;
    delete_key();                                   /* a leftover from a failed run */
    if (!CryptAcquireContextW(&prov, CONTAINER, MS_ENH_RSA_AES_PROV_W, PROV_RSA_AES,
                              CRYPT_MACHINE_KEYSET | CRYPT_NEWKEYSET | CRYPT_SILENT))
        return NULL;
    BOOL gen = CryptGenKey(prov, AT_SIGNATURE, 2048u << 16, &key);
    if (gen)
        CryptDestroyKey(key);
    CryptReleaseContext(prov, 0);
    if (!gen)
        return NULL;

    BYTE name[256];
    CERT_NAME_BLOB subject = { sizeof name, name };
    if (!CertStrToNameW(X509_ASN_ENCODING, L"CN=\"" CERT_CN L"\"", CERT_X500_NAME_STR, NULL,
                        name, &subject.cbData, NULL))
        return NULL;
    LPSTR usages[] = { szOID_PKIX_KP_CODE_SIGNING };
    CERT_ENHKEY_USAGE eku = { 1, usages };
    BYTE ekub[64];
    DWORD ekun = sizeof ekub;
    if (!CryptEncodeObject(X509_ASN_ENCODING, X509_ENHANCED_KEY_USAGE, &eku, ekub, &ekun))
        return NULL;
    CERT_EXTENSION ext = { szOID_ENHANCED_KEY_USAGE, FALSE, { ekun, ekub } };
    CERT_EXTENSIONS exts = { 1, &ext };
    CRYPT_KEY_PROV_INFO kpi = { CONTAINER, MS_ENH_RSA_AES_PROV_W, PROV_RSA_AES,
                                CRYPT_MACHINE_KEYSET, 0, NULL, AT_SIGNATURE };
    CRYPT_ALGORITHM_IDENTIFIER alg = { szOID_RSA_SHA256RSA, { 0, NULL } };
    SYSTEMTIME end;
    GetSystemTime(&end);
    end.wYear += 30;
    if (end.wMonth == 2 && end.wDay == 29)
        end.wDay = 28;
    return CertCreateSelfSignCertificate(0, &subject, 0, &kpi, &alg, NULL, &end, &exts);
}

/* pakon-winusb.cat: the INF's SHA-1 hash with the attributes Inf2Cat writes. */
static BOOL make_catalog(void)
{
    const DWORD attr = CRYPTCAT_ATTR_AUTHENTICATED | CRYPTCAT_ATTR_NAMEASCII |
                       CRYPTCAT_ATTR_DATAASCII;
    GUID inf_sip = { 0xDE351A42, 0x8E59, 0x11D0,
                     { 0x8C, 0x47, 0x00, 0xC0, 0x4F, 0xC2, 0x95, 0xEE } };
    BYTE hash[20];
    DWORD nhash = sizeof hash;
    HANDLE f = CreateFileW(inf_path, GENERIC_READ, FILE_SHARE_READ, NULL, OPEN_EXISTING, 0, NULL);
    if (f == INVALID_HANDLE_VALUE)
        return FALSE;
    BOOL ok = CryptCATAdminCalcHashFromFileHandle(f, &nhash, hash, 0) && nhash == sizeof hash;
    CloseHandle(f);
    if (!ok)
        return FALSE;

    DeleteFileW(cat_path);
    HANDLE cat = CryptCATOpen(cat_path, CRYPTCAT_OPEN_CREATENEW, 0, 0, 0);
    if (cat == INVALID_HANDLE_VALUE || !cat)
        return FALSE;
    wchar_t tag[16];
    for (int i = 0; i < 2; i++) {
        _snwprintf(tag, ARRAYSIZE(tag), L"HWID%d", i + 1);
        ok &= CryptCATPutCatAttrInfo(cat, tag, attr, 2 * (wcslen(HWIDS[i]) + 1),
                                     (BYTE *)HWIDS[i]) != NULL;
    }
    wchar_t os[] = L"10_X64,10_ARM64";
    ok &= CryptCATPutCatAttrInfo(cat, L"OS", attr, sizeof os, (BYTE *)os) != NULL;

    /* the member: the hash in an indirect-data blob with the obsolete link */
    SPC_LINK link = { 0 };
    link.dwLinkChoice = SPC_FILE_LINK_CHOICE;
    link.pwszFile = L"<<<Obsolete>>>";
    BYTE linkb[64];
    DWORD linkn = sizeof linkb;
    ok &= CryptEncodeObject(X509_ASN_ENCODING, SPC_CAB_DATA_OBJID, &link, linkb, &linkn);
    SIP_INDIRECT_DATA sip = { 0 };
    sip.Data.pszObjId = SPC_CAB_DATA_OBJID;
    sip.Data.Value.cbData = linkn;
    sip.Data.Value.pbData = linkb;
    sip.DigestAlgorithm.pszObjId = szOID_OIWSEC_sha1;
    sip.Digest.cbData = sizeof hash;
    sip.Digest.pbData = hash;
    wchar_t hex[2 * sizeof hash + 1];
    for (unsigned i = 0; i < sizeof hash; i++)
        _snwprintf(hex + 2 * i, 3, L"%02X", hash[i]);
    CRYPTCATMEMBER *m = ok ? CryptCATPutMemberInfo(cat, NULL, hex, &inf_sip, 0x200,
                                                   sizeof sip, (BYTE *)&sip) : NULL;
    wchar_t file[] = INF_NAME, osattr[] = L"2:6.1,2:6.2,2:6.3,2:10.0";
    ok = m && CryptCATPutAttrInfo(cat, m, L"File", attr, sizeof file, (BYTE *)file) &&
         CryptCATPutAttrInfo(cat, m, L"OSAttr", attr, sizeof osattr, (BYTE *)osattr) &&
         CryptCATPersistStore(cat);
    CryptCATClose(cat);
    return ok;
}

static HRESULT sign_catalog(PCCERT_CONTEXT cert)
{
    HMODULE ms = LoadLibraryW(L"mssign32.dll");
    SignerSignEx_fn sign = ms ? (SignerSignEx_fn)(void *)GetProcAddress(ms, "SignerSignEx") : NULL;
    SignerFreeSignerContext_fn free_ctx = ms ?
        (SignerFreeSignerContext_fn)(void *)GetProcAddress(ms, "SignerFreeSignerContext") : NULL;
    if (!sign)
        return HRESULT_FROM_WIN32(GetLastError());
    DWORD index = 0;
    SIGNER_FILE_INFO file = { sizeof file, cat_path, NULL };
    SIGNER_SUBJECT_INFO subject = { sizeof subject, &index, SIGNER_SUBJECT_FILE, { &file } };
    SIGNER_CERT_STORE_INFO store = { sizeof store, cert, SIGNER_CERT_POLICY_CHAIN, NULL };
    SIGNER_CERT signer = { sizeof signer, SIGNER_CERT_STORE, { .pCertStoreInfo = &store }, NULL };
    SIGNER_SIGNATURE_INFO info = { sizeof info, CALG_SHA_256, SIGNER_NO_ATTR, NULL, NULL, NULL };
    SIGNER_CONTEXT *ctx = NULL;
    HRESULT hr = sign(0, &subject, &signer, &info, NULL, NULL, NULL, NULL, &ctx);
    if (ctx && free_ctx)
        free_ctx(ctx);
    return hr;
}

static int install(void)
{
    if (GetFileAttributesW(inf_path) == INVALID_FILE_ATTRIBUTES)
        return report(0, L"pakon-winusb.inf is missing next to pakon_driver.exe.",
                      GetLastError());
    PCCERT_CONTEXT cert = make_cert();
    if (!cert)
        return report(0, L"Could not create the signing certificate.", GetLastError());
    DWORD err = 0;
    const wchar_t *step = NULL;
    HRESULT hr;
    if (!make_catalog()) {
        err = GetLastError();
        step = L"Could not create the driver catalog.";
    } else if (FAILED(hr = sign_catalog(cert))) {
        err = (DWORD)hr;
        step = L"Could not sign the driver catalog.";
    } else if (!machine_stores(cert)) {
        err = GetLastError();
        step = L"Could not add the certificate to this computer's trusted stores.";
    }
    CertFreeCertificateContext(cert);
    delete_key();                                   /* signs nothing else, ever */
    if (step)
        return report(0, step, err);

    HMODULE nd = LoadLibraryW(L"newdev.dll");
    DiInstallDriverW_fn di = nd ?
        (DiInstallDriverW_fn)(void *)GetProcAddress(nd, "DiInstallDriverW") : NULL;
    BOOL reboot = FALSE;
    /* into the driver store, and onto a connected scanner (replacing Kodak's
     * driver, which a better-ranked match would otherwise keep) */
    if (!di || !di(NULL, inf_path, DIIRFLAG_FORCE_INF, &reboot)) {
        err = di ? GetLastError() : ERROR_PROC_NOT_FOUND;
        /* no scanner connected: staging it is enough, Windows uses it when
         * one appears */
        if (!SetupCopyOEMInfW(inf_path, NULL, SPOST_PATH, 0, NULL, 0, NULL, NULL))
            return report(0, L"Could not add the driver to Windows' driver store.",
                          GetLastError());
    }
    HANDLE m = CreateFileW(marker, GENERIC_WRITE, 0, NULL, CREATE_ALWAYS, 0, NULL);
    if (m != INVALID_HANDLE_VALUE)
        CloseHandle(m);
    return report(1, reboot
        ? L"The scanner's USB driver is set up. Restart Windows to finish."
        : L"The scanner's USB driver is set up. Turn the scanner off and on, then "
          L"start Pakon.", 0);
}

static int uninstall(void)
{
    HMODULE nd = LoadLibraryW(L"newdev.dll");
    DiUninstallDriverW_fn du = nd ?
        (DiUninstallDriverW_fn)(void *)GetProcAddress(nd, "DiUninstallDriverW") : NULL;
    BOOL reboot = FALSE;
    BOOL removed = du && du(NULL, inf_path, 0, &reboot);
    DWORD err = removed ? 0 : (du ? GetLastError() : ERROR_PROC_NOT_FOUND);
    machine_stores(NULL);
    delete_key();
    DeleteFileW(marker);
    if (!removed && err != ERROR_FILE_NOT_FOUND && err != ERROR_NOT_FOUND)
        return report(0, L"Could not remove the driver package (the certificate is "
                      L"removed).", err);
    return report(1, L"The scanner's USB driver and its certificate are removed.", 0);
}

int WINAPI wWinMain(HINSTANCE inst, HINSTANCE prev, PWSTR cmd, int show)
{
    (void)inst; (void)prev; (void)cmd; (void)show;
    int argc;
    wchar_t **argv = CommandLineToArgvW(GetCommandLineW(), &argc);
    const wchar_t *verb = argc > 1 ? argv[1] : L"";
    for (int i = 2; i < argc; i++)
        quiet |= !wcscmp(argv[i], L"--quiet");

    DWORD n = GetModuleFileNameW(NULL, dir, MAX_PATH);
    wchar_t *slash = n && n < MAX_PATH ? wcsrchr(dir, L'\\') : NULL;
    if (!slash)
        return report(0, L"Cannot find the folder of pakon_driver.exe.", GetLastError());
    *slash = 0;
    _snwprintf(inf_path, MAX_PATH, L"%ls\\" INF_NAME, dir);
    _snwprintf(cat_path, MAX_PATH, L"%ls\\" CAT_NAME, dir);
    _snwprintf(marker, MAX_PATH, L"%ls\\" MARKER, dir);

    if (wcscmp(verb, L"install") && wcscmp(verb, L"uninstall")) {
        MessageBoxW(NULL, L"Usage: pakon_driver.exe install|uninstall [--quiet]",
                    L"Pakon USB driver", MB_OK);
        return 2;
    }
    if (!is_admin())
        return report(0, L"Setting up the USB driver needs administrator rights.",
                      ERROR_ACCESS_DENIED);
    return wcscmp(verb, L"install") ? uninstall() : install();
}
