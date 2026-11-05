// Windows or Mac. Both send the same User-Agent, but their web libraries differ
// (read on GitHub's windows-2025 and macos-15 runners, 5 Nov 2026): Apple's
// URLSession adds Accept and Accept-Language by itself, and Windows.Web.Http
// sends neither. Anything else stays unknown rather than guessed.

export function systemOf(headers) {
    const accept = headers.get('accept');
    const language = headers.get('accept-language');
    if (accept === null && language === null) return 'windows';
    if (accept === '*/*' && language !== null) return 'mac';
    return null;
}
