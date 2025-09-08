Get-Content ".env" | ForEach-Object {
    if ($_ -match "^\s*#" -or $_ -match "^\s*$") { return }

    $pair = $_ -split "="
    $key = $pair[0].Trim()
    $value = ($pair[1..($pair.Length - 1)] -join "=").Trim()

    [System.Environment]::SetEnvironmentVariable($key, $value, "Process")
}
