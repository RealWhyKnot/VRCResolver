@{
    Severity            = @('Error', 'Warning', 'Information', 'ParseError')
    CustomRulePath      = @('PSStyleRules.psm1')
    IncludeDefaultRules = $true
    ExcludeRules        = @(
        'PSAvoidUsingWriteHost'
        'PSProvideCommentHelp'
        'PSUseShouldProcessForStateChangingFunctions'
    )
    Rules               = @{
        PSUseCompatibleSyntax                     = @{
            Enable         = $true
            TargetVersions = @('5.1', '7.4')
        }
        PSUseCompatibleCommands                   = @{
            Enable         = $true
            TargetProfiles = @(
                'win-8_x64_10.0.17763.0_5.1.17763.316_x64_4.0.30319.42000_framework'
                'win-8_x64_10.0.17763.0_7.0.0_x64_3.1.2_core'
            )
        }
        PSUseCompatibleTypes                      = @{
            Enable         = $true
            TargetProfiles = @(
                'win-8_x64_10.0.17763.0_5.1.17763.316_x64_4.0.30319.42000_framework'
                'win-8_x64_10.0.17763.0_7.0.0_x64_3.1.2_core'
            )
        }
        PSUseConsistentParametersKind             = @{
            Enable         = $true
            ParametersKind = 'ParamBlock'
        }
        PSAlignAssignmentStatement                = @{
            Enable         = $true
            CheckHashtable = $true
        }
        PSPlaceOpenBrace                          = @{
            Enable             = $true
            OnSameLine         = $true
            NewLineAfter       = $true
            IgnoreOneLineBlock = $true
        }
        PSPlaceCloseBrace                         = @{
            Enable             = $true
            NewLineAfter       = $false
            IgnoreOneLineBlock = $true
            NoEmptyLineBefore  = $false
        }
        PSUseConsistentIndentation                = @{
            Enable              = $true
            Kind                = 'space'
            IndentationSize     = 4
            PipelineIndentation = 'IncreaseIndentationForFirstPipeline'
        }
        PSUseConsistentWhitespace                 = @{
            Enable                                  = $true
            CheckInnerBrace                         = $true
            CheckOpenBrace                          = $true
            CheckOpenParen                          = $true
            CheckOperator                           = $true
            CheckPipe                               = $true
            CheckPipeForRedundantWhitespace         = $true
            CheckSeparator                          = $true
            CheckParameter                          = $true
            IgnoreAssignmentOperatorInsideHashTable = $true
        }
        PSUseCorrectCasing                        = @{ Enable = $true }
        PSAvoidSemicolonsAsLineTerminators        = @{ Enable = $true }
        PSAvoidExclaimOperator                    = @{ Enable = $true }
        PSAvoidLongLines                          = @{
            Enable            = $true
            MaximumLineLength = 120
        }
        PSAvoidUsingDoubleQuotesForConstantString = @{ Enable = $true }
        PSUseConsistentParameterSetName           = @{ Enable = $true }
        PSUseSingleValueFromPipelineParameter     = @{ Enable = $true }
    }
}
