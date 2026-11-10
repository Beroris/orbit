# Windows Settings: an example Orbit extension. Orbit sends one JSON message per line on standard
# input, and reads one per line from standard output. docs/EXTENSIONS.md describes them.

$ErrorActionPreference = 'Stop'

# Orbit speaks UTF-8. Reading and writing through UTF-8 streams keeps any text intact, whatever
# code page the console has.
$utf8 = [Text.UTF8Encoding]::new($false)
$in = [IO.StreamReader]::new([Console]::OpenStandardInput(), $utf8)
$out = [IO.StreamWriter]::new([Console]::OpenStandardOutput(), $utf8)
$out.AutoFlush = $true

$pages = Get-Content -Raw -Encoding UTF8 (Join-Path $PSScriptRoot 'pages.json') | ConvertFrom-Json

function Send($message) {
    $out.WriteLine(($message | ConvertTo-Json -Depth 8 -Compress))
}

# Every word typed has to be in the page's name, keywords or section. Pages whose name starts with
# the first word come first, then those with it in their name, then in their keywords.
function Find-Pages([string]$query) {
    $words = @($query.ToLowerInvariant().Split([char[]]' ', [StringSplitOptions]::RemoveEmptyEntries))
    $found = foreach ($page in $pages) {
        $name = $page.name.ToLowerInvariant()
        $keywords = "$($page.keywords)".ToLowerInvariant()
        $text = "$name $keywords $($page.section)".ToLowerInvariant()
        $missing = @($words | Where-Object { -not $text.Contains($_) })
        if ($missing.Count -gt 0) { continue }
        $rank = 3
        if ($words.Count -eq 0) { $rank = 0 }
        elseif ($name.StartsWith($words[0])) { $rank = 0 }
        elseif ($name.Contains($words[0])) { $rank = 1 }
        elseif ($keywords.Contains($words[0])) { $rank = 2 }
        [pscustomobject]@{ rank = $rank; page = $page }
    }
    foreach ($match in @($found | Sort-Object rank, { $_.page.name })) {
        $page = $match.page
        [ordered]@{
            id        = $page.uri
            title     = $page.name
            subtitle  = $page.uri
            accessory = $page.section
            actions   = @(
                [ordered]@{ title = 'Open'; type = 'open'; target = $page.uri },
                [ordered]@{ title = 'Copy Link'; type = 'copy'; text = $page.uri }
            )
        }
    }
}

while ($null -ne ($line = $in.ReadLine())) {
    try { $message = $line | ConvertFrom-Json } catch { continue }
    if ($message.type -eq 'search') {
        $query = "$($message.query)".Trim()
        Send ([ordered]@{ type = 'items'; id = $message.id; items = @(Find-Pages $query); empty = "No Settings page matches `"$query`"" })
    } elseif ($message.type -eq 'run' -or $message.type -eq 'action') {
        Send ([ordered]@{ type = 'done'; id = $message.id })
    }
}
