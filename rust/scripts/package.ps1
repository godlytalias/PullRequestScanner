[CmdletBinding()]
param(
    [ValidateSet("x64", "arm64", "all")]
    [string]$Architecture = "all",

    [ValidateSet("debug", "release")]
    [string]$Configuration = "release"
)

$ErrorActionPreference = "Stop"

$repositoryRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path
$manifestTemplate = Join-Path $repositoryRoot "rust\package\AppxManifest.template.xml"
$assetSource = Join-Path $repositoryRoot "src\PullRequestScanner.App\Assets"
$packageRoot = Join-Path $repositoryRoot "target\package"
$sdkPackageCache = Join-Path $repositoryRoot "target\naui-winappsdk-packages"
$cargo = Join-Path $HOME ".cargo\bin\cargo.exe"
$winapp = Get-Command "winapp.exe" -ErrorAction Stop
$version = "1.0.0.0"
$sdkPackages = @(
    @{ Id = "microsoft.windowsappsdk.foundation"; Version = "2.3.12" },
    @{ Id = "microsoft.windowsappsdk.interactiveexperiences"; Version = "2.1.9" },
    @{ Id = "microsoft.windowsappsdk.winui"; Version = "2.3.9" },
    @{ Id = "microsoft.windowsappsdk.dwrite"; Version = "2.1.0" }
)

$architectures = if ($Architecture -eq "all") {
    @("x64", "arm64")
}
else {
    @($Architecture)
}

New-Item -ItemType Directory -Force -Path $packageRoot | Out-Null

$nugetRoot = Join-Path $HOME ".nuget\packages"
New-Item -ItemType Directory -Force -Path $sdkPackageCache | Out-Null
$allSdkPackagesCached = $true
foreach ($package in $sdkPackages) {
    $name = "$($package.Id).$($package.Version).nupkg"
    $destination = Join-Path $sdkPackageCache $name
    if (!(Test-Path -LiteralPath $destination)) {
        $source = Join-Path $nugetRoot "$($package.Id)\$($package.Version)\$name"
        if (Test-Path -LiteralPath $source) {
            Copy-Item -LiteralPath $source -Destination $destination
        }
        else {
            $allSdkPackagesCached = $false
        }
    }
}
if ($allSdkPackagesCached) {
    $env:NAUI_WINAPPSDK_PACKAGES = $sdkPackageCache
}

foreach ($currentArchitecture in $architectures) {
    $rustTarget = if ($currentArchitecture -eq "x64") {
        "x86_64-pc-windows-msvc"
    }
    else {
        "aarch64-pc-windows-msvc"
    }
    $profile = $Configuration.ToLowerInvariant()
    $executable = Join-Path $repositoryRoot "target\$rustTarget\$profile\prs-app.exe"
    $runtimeBundle = Join-Path $repositoryRoot "target\$rustTarget\$profile\naui-winappsdk"
    $layout = Join-Path $packageRoot "layout-$currentArchitecture"
    $assetLayout = Join-Path $layout "Assets"
    $manifest = Join-Path $layout "AppxManifest.xml"
    $output = Join-Path $packageRoot "PullRequestScanner_$version`_$currentArchitecture.msix"

    Push-Location $repositoryRoot
    try {
        & $cargo build "--$profile" -p prs-app --target $rustTarget --features self-contained
        if ($LASTEXITCODE -ne 0) {
            throw "Rust $currentArchitecture build failed with exit $LASTEXITCODE."
        }
    }
    finally {
        Pop-Location
    }

    if (Test-Path -LiteralPath $layout) {
        Remove-Item -LiteralPath $layout -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $assetLayout | Out-Null
    Copy-Item -LiteralPath $executable -Destination $layout
    if (!(Test-Path -LiteralPath $runtimeBundle)) {
        throw "Windows App SDK bundle was not produced at '$runtimeBundle'."
    }
    Copy-Item -Path (Join-Path $runtimeBundle "*") -Destination $layout -Recurse -Force
    foreach ($asset in @(
        "StoreLogo.png",
        "Square150x150Logo.scale-200.png",
        "Square44x44Logo.scale-200.png",
        "Wide310x150Logo.scale-200.png",
        "SplashScreen.scale-200.png"
    )) {
        Copy-Item -LiteralPath (Join-Path $assetSource $asset) -Destination $assetLayout
    }
    $qualifiedAssetNames = @{
        "SplashScreen.scale-200.png" = "SplashScreen.png"
        "Square44x44Logo.scale-200.png" = "Square44x44Logo.png"
        "Square150x150Logo.scale-200.png" = "Square150x150Logo.png"
        "Wide310x150Logo.scale-200.png" = "Wide310x150Logo.png"
    }
    foreach ($asset in $qualifiedAssetNames.GetEnumerator()) {
        Copy-Item `
            -LiteralPath (Join-Path $assetLayout $asset.Key) `
            -Destination (Join-Path $assetLayout $asset.Value)
    }

    $manifestText = (Get-Content -LiteralPath $manifestTemplate -Raw).Replace(
        "__VERSION__",
        $version
    ).Replace(
        "__ARCH__",
        $currentArchitecture
    )
    Set-Content -LiteralPath $manifest -Value $manifestText -Encoding utf8NoBOM

    & $winapp.Source package $layout `
        --manifest $manifest `
        --exe "prs-app.exe" `
        --output $output `
        --skip-pri `
        --no-sign
    if ($LASTEXITCODE -ne 0) {
        throw "MSIX packaging for $currentArchitecture failed with exit $LASTEXITCODE."
    }
}
