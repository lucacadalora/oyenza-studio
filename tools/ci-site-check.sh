#!/usr/bin/env bash
# Keep the website in step with releases (AGENTS.md, "Every release updates the
# website"). The page's fallback version, <span data-release="version">, says
# which release site/index.html describes.
#
# Usage:   tools/ci-site-check.sh version           print that version
#          tools/ci-site-check.sh gate [RELEASED]   pages.yml, before deploying: hold the
#                                                   page while it names an unreleased version
#          tools/ci-site-check.sh close             pages.yml, after deploying: close the
#                                                   "not updated" issues the page now answers
#          tools/ci-site-check.sh release VERSION   ci.yml, once VERSION is published: put the
#                                                   page live if it names VERSION, otherwise
#                                                   open an issue
# Needs:   gh with GH_TOKEN, and GITHUB_REPOSITORY (all but `version`)
#
# RELEASED is the version the release run has just published. It counts as
# released even while the GitHub API doesn't list it as the latest yet.
set -euo pipefail
cd "$(dirname "$0")/.."

PAGE=site/index.html
TITLE="Landing page not updated for v"
REPO=${GITHUB_REPOSITORY:-lucacadalora/poni}
SERVER=${GITHUB_SERVER_URL:-https://github.com}

die() {
    echo "error: $*" >&2
    exit 2
}

is_version() {
    [[ $1 =~ ^[0-9]+(\.[0-9]+)*$ ]]
}

# newer A B: version A is later than version B.
newer() {
    [ "$1" != "$2" ] && [ "$(printf '%s\n%s\n' "$1" "$2" | sort -V | tail -n 1)" = "$1" ]
}

page_version() {
    sed -n '/data-release="version">/{s/.*data-release="version">\([^<]*\)<.*/\1/p;q}' "$PAGE"
}

page_url() {
    local owner=${REPO%%/*}
    echo "https://${owner,,}.github.io/${REPO#*/}/"
}

summary() {
    printf '%s\n' "$*" >> "${GITHUB_STEP_SUMMARY:-/dev/stderr}"
}

# The open "not updated" issues, one "NUMBER<TAB>VERSION" line each.
open_issues() {
    gh api "repos/$REPO/issues?state=open&per_page=100" \
        --jq '.[] | select(.pull_request == null) | "\(.number)\t\(.title)"' |
        while IFS=$'\t' read -r number title; do
            case $title in
                "$TITLE"*) printf '%s\t%s\n' "$number" "${title#"$TITLE"}" ;;
            esac
        done
}

gate() {
    local released=${1:-} page latest deploy=true
    if [ -n "$released" ] && ! is_version "$released"; then
        echo "::warning::Ignoring RELEASED=\"$released\", which is not a version."
        released=
    fi
    page=$(page_version)
    latest=$(gh api "repos/$REPO/releases/latest" --jq .tag_name 2> /dev/null) || latest=
    latest=${latest#v}
    is_version "$latest" || latest=
    if [ -n "$released" ] && { [ -z "$latest" ] || newer "$released" "$latest"; }; then
        latest=$released
    fi

    if [ -z "$page" ]; then
        echo "::warning file=$PAGE::No <span data-release=\"version\"> on the page, so it isn't checked against the releases."
    elif [ -z "$latest" ]; then
        echo "::warning::Couldn't read the latest release, so the page isn't checked against it."
    elif newer "$page" "$latest"; then
        deploy=false
        echo "::notice file=$PAGE::Holding the page: it describes $page and the latest release is $latest. The ci run for v$page puts it live once that release is published."
        summary "**Held.** The page describes $page, but the latest release is $latest. The \`ci\` run for v$page puts it live once that release is published."
    elif newer "$latest" "$page"; then
        echo "::warning file=$PAGE::The page still describes $page, but $latest is out. Update it for $latest (see AGENTS.md)."
        summary "Deploying, but the page still describes $page while $latest is out. Update it for $latest (see AGENTS.md)."
    else
        summary "Deploying the page for $page, the latest release."
    fi
    echo "deploy=$deploy" >> "${GITHUB_OUTPUT:-/dev/stdout}"
}

close_issues() {
    local page issues number version run
    page=$(page_version)
    [ -n "$page" ] || return 0
    if ! issues=$(open_issues); then
        echo "::warning::Couldn't list the open issues, so none were closed."
        return 0
    fi
    run="$SERVER/$REPO/actions/runs/${GITHUB_RUN_ID:-}"
    while IFS=$'\t' read -r number version; do
        is_version "${version:-}" || continue
        newer "$version" "$page" && continue
        if gh api "repos/$REPO/issues/$number/comments" \
            -f body="The website now describes $page: $(page_url) ([Website run]($run))." > /dev/null &&
            gh api -X PATCH "repos/$REPO/issues/$number" -f state=closed -f state_reason=completed > /dev/null; then
            echo "Closed #$number (v$version): the page describes $page."
            summary "Closed #$number: the page now describes $page."
        else
            echo "::warning::Couldn't close #$number (v$version)."
        fi
    done <<< "$issues"
}

issue_body() {
    local version=$1 page=$2
    cat << EOF
[v$version]($SERVER/$REPO/releases/tag/v$version) is published, but [the website]($(page_url)) still describes ${page:-an older version}. Every release updates the website too ([AGENTS.md]($SERVER/$REPO/blob/main/AGENTS.md#every-release-updates-the-website)):

- [ ] \`<span data-release="version">\` in \`site/index.html\` says $version, and the size next to it is right
- [ ] a "New in $version" section for each change people will notice
- [ ] copy this release makes out of date, such as "coming soon" lines
- [ ] screenshots this release changed (\`tools/site_shots.py\`)
- [ ] the share card, if it changed (\`tools/make_share_card.py\`, then bump \`?v=\`)

This issue closes itself when the Website workflow deploys a page that says $version.
EOF
}

check_release() {
    local version=${1#v} page number
    is_version "$version" || die "release: \"$1\" is not a version"
    page=$(page_version)

    # The page names this release (or a later one): it was held until now, so put it live.
    if [ -n "$page" ] && ! newer "$version" "$page"; then
        gh api -X POST "repos/$REPO/actions/workflows/pages.yml/dispatches" \
            -f ref=main -f "inputs[released]=$version" > /dev/null
        echo "The page on main describes $page: started the Website workflow to put it live."
        summary "The page on main describes $page: started the Website workflow to put it live."
        if ! grep -qF "New in $version" "$PAGE"; then
            echo "::notice file=$PAGE::The page has no \"New in $version\" section. That's fine if this release has nothing new to show."
        fi
        return 0
    fi

    echo "::warning file=$PAGE::v$version is out, but the page on main still describes ${page:-no version}. Update the website (see AGENTS.md)."
    number=$(open_issues | awk -F '\t' -v v="$version" '$2 == v && !n { n = $1 } END { if (n) print n }')
    if [ -n "$number" ]; then
        summary "v$version is out, but the page still describes ${page:-no version}. Issue #$number is already open for it."
        return 0
    fi
    number=$(gh api "repos/$REPO/issues" -f title="$TITLE$version" -f body="$(issue_body "$version" "$page")" --jq .number)
    echo "Opened issue #$number."
    summary "v$version is out, but the page still describes ${page:-no version}. Opened issue #$number."
}

case ${1:-} in
    version)
        v=$(page_version)
        [ -n "$v" ] || die "no <span data-release=\"version\"> in $PAGE"
        echo "$v"
        ;;
    gate) gate "${2:-}" ;;
    close) close_issues ;;
    release)
        [ -n "${2:-}" ] || die "usage: $0 release VERSION"
        check_release "$2"
        ;;
    *) die "usage: $0 version | gate [RELEASED] | close | release VERSION" ;;
esac
