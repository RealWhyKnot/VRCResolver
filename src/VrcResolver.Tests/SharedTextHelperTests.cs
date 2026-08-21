using VrcResolver.Shared;
using Xunit;

namespace VrcResolver.Tests;

// LogUtil.BareHost and Base64UrlText replaced per-project private copies; these
// pin the unified behavior (www-strip, padded-and-unpadded decode tolerance).
public class SharedTextHelperTests
{
    [Theory]
    [InlineData("https://www.youtube.com/watch?v=abc", "youtube.com")]
    [InlineData("https://youtube.com/watch?v=abc", "youtube.com")]
    [InlineData("https://WWW.Example.COM/x", "example.com")]  // Uri.Host lowercases
    [InlineData("https://wwwx.example.com/x", "wwwx.example.com")]
    [InlineData("not a url", "?")]
    [InlineData("", "?")]
    [InlineData(null, "?")]
    public void BareHost_StripsWwwAndFallsBackToQuestionMark(string? url, string expected)
    {
        Assert.Equal(expected, LogUtil.BareHost(url));
    }

    [Fact]
    public void Base64UrlText_RoundTrips_WithUrlSafeAlphabetAndNoPadding()
    {
        const string value = "https://cdn.example.com/seg?sig=a+b/c=&t=1";
        string encoded = Base64UrlText.Encode(value);
        Assert.DoesNotContain("+", encoded);
        Assert.DoesNotContain("/", encoded);
        Assert.DoesNotContain("=", encoded);
        Assert.True(Base64UrlText.TryDecode(encoded, out string decoded));
        Assert.Equal(value, decoded);
    }

    [Fact]
    public void Base64UrlText_TryDecode_AcceptsPaddedInput()
    {
        // Wire-adjacent tolerance the BCL Base64Url type does not promise.
        Assert.True(Base64UrlText.TryDecode("aGk=", out string decoded));
        Assert.Equal("hi", decoded);
    }

    [Theory]
    [InlineData("!!!!")]
    [InlineData(null)]
    [InlineData("")]
    public void Base64UrlText_TryDecode_RejectsInvalidInput(string? encoded)
    {
        Assert.False(Base64UrlText.TryDecode(encoded, out _));
    }
}
