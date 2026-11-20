var MediaChrome=(()=>{var eo=Object.defineProperty;var fa=Object.getOwnPropertyDescriptor;var va=Object.getOwnPropertyNames;var Ta=Object.prototype.hasOwnProperty;var to=(i,t)=>{for(var e in t)eo(i,e,{get:t[e],enumerable:!0})},Aa=(i,t,e,r)=>{if(t&&typeof t=="object"||typeof t=="function")for(let o of va(t))!Ta.call(i,o)&&o!==e&&eo(i,o,{get:()=>t[o],enumerable:!(r=fa(t,o))||r.enumerable});return i};var ya=i=>Aa(eo({},"__esModule",{value:!0}),i);var io=(i,t,e)=>{if(!t.has(i))throw TypeError("Cannot "+e)};var s=(i,t,e)=>(io(i,t,"read from private field"),e?e.call(i):t.get(i)),u=(i,t,e)=>{if(t.has(i))throw TypeError("Cannot add the same private member more than once");t instanceof WeakSet?t.add(i):t.set(i,e)},h=(i,t,e,r)=>(io(i,t,"write to private field"),r?r.call(i,e):t.set(i,e),e);var ln=(i,t,e,r)=>({set _(o){h(i,t,o,e)},get _(){return s(i,t,r)}}),f=(i,t,e)=>(io(i,t,"access private method"),e);var Ol={};to(Ol,{MediaAirplayButton:()=>Es,MediaCaptionsButton:()=>vs,MediaCastButton:()=>As,MediaChromeButton:()=>ms,MediaChromeDialog:()=>Un,MediaChromeRange:()=>Ls,MediaContainer:()=>Dn,MediaControlBar:()=>ks,MediaController:()=>ls,MediaDurationDisplay:()=>Ds,MediaErrorDialog:()=>Cs,MediaFullscreenButton:()=>Ps,MediaGestureReceiver:()=>Jt,MediaKeyboardShortcutsDialog:()=>Hn,MediaLiveButton:()=>Ns,MediaLoadingIndicator:()=>Fs,MediaLoopButton:()=>ca,MediaMuteButton:()=>Vs,MediaPipButton:()=>qs,MediaPlayButton:()=>Qs,MediaPlaybackRateButton:()=>Ws,MediaPosterImage:()=>js,MediaPreviewChapterDisplay:()=>zs,MediaPreviewThumbnail:()=>Ui,MediaPreviewTimeDisplay:()=>Zs,MediaSeekBackwardButton:()=>Xs,MediaSeekForwardButton:()=>Js,MediaTextDisplay:()=>Rs,MediaTimeDisplay:()=>ra,MediaTimeRange:()=>da,MediaTooltip:()=>gi,MediaVolumeRange:()=>la,constants:()=>tr,t:()=>g,timeUtils:()=>or});var tr={};to(tr,{AttributeToStateChangeEventMap:()=>ro,AvailabilityStates:()=>Y,MediaStateChangeEvents:()=>Se,MediaStateReceiverAttributes:()=>k,MediaUIAttributes:()=>a,MediaUIEvents:()=>E,MediaUIProps:()=>er,PointerTypes:()=>zt,ReadyStates:()=>Ma,StateChangeEventToAttributeMap:()=>Ia,StreamTypes:()=>j,TextTrackKinds:()=>W,TextTrackModes:()=>fe,VolumeLevels:()=>_a,WebkitPresentationModes:()=>oo});var E={MEDIA_PLAY_REQUEST:"mediaplayrequest",MEDIA_PAUSE_REQUEST:"mediapauserequest",MEDIA_MUTE_REQUEST:"mediamuterequest",MEDIA_UNMUTE_REQUEST:"mediaunmuterequest",MEDIA_LOOP_REQUEST:"medialooprequest",MEDIA_VOLUME_REQUEST:"mediavolumerequest",MEDIA_SEEK_REQUEST:"mediaseekrequest",MEDIA_AIRPLAY_REQUEST:"mediaairplayrequest",MEDIA_ENTER_FULLSCREEN_REQUEST:"mediaenterfullscreenrequest",MEDIA_EXIT_FULLSCREEN_REQUEST:"mediaexitfullscreenrequest",MEDIA_PREVIEW_REQUEST:"mediapreviewrequest",MEDIA_ENTER_PIP_REQUEST:"mediaenterpiprequest",MEDIA_EXIT_PIP_REQUEST:"mediaexitpiprequest",MEDIA_ENTER_CAST_REQUEST:"mediaentercastrequest",MEDIA_EXIT_CAST_REQUEST:"mediaexitcastrequest",MEDIA_SHOW_TEXT_TRACKS_REQUEST:"mediashowtexttracksrequest",MEDIA_HIDE_TEXT_TRACKS_REQUEST:"mediahidetexttracksrequest",MEDIA_SHOW_SUBTITLES_REQUEST:"mediashowsubtitlesrequest",MEDIA_DISABLE_SUBTITLES_REQUEST:"mediadisablesubtitlesrequest",MEDIA_TOGGLE_SUBTITLES_REQUEST:"mediatogglesubtitlesrequest",MEDIA_PLAYBACK_RATE_REQUEST:"mediaplaybackraterequest",MEDIA_RENDITION_REQUEST:"mediarenditionrequest",MEDIA_AUDIO_TRACK_REQUEST:"mediaaudiotrackrequest",MEDIA_SEEK_TO_LIVE_REQUEST:"mediaseektoliverequest",REGISTER_MEDIA_STATE_RECEIVER:"registermediastatereceiver",UNREGISTER_MEDIA_STATE_RECEIVER:"unregistermediastatereceiver"},k={MEDIA_CHROME_ATTRIBUTES:"mediachromeattributes",MEDIA_CONTROLLER:"mediacontroller"},er={MEDIA_AIRPLAY_UNAVAILABLE:"mediaAirplayUnavailable",MEDIA_AUDIO_TRACK_ENABLED:"mediaAudioTrackEnabled",MEDIA_AUDIO_TRACK_LIST:"mediaAudioTrackList",MEDIA_AUDIO_TRACK_UNAVAILABLE:"mediaAudioTrackUnavailable",MEDIA_BUFFERED:"mediaBuffered",MEDIA_CAST_UNAVAILABLE:"mediaCastUnavailable",MEDIA_CHAPTERS_CUES:"mediaChaptersCues",MEDIA_CURRENT_TIME:"mediaCurrentTime",MEDIA_DURATION:"mediaDuration",MEDIA_ENDED:"mediaEnded",MEDIA_ERROR:"mediaError",MEDIA_ERROR_CODE:"mediaErrorCode",MEDIA_ERROR_MESSAGE:"mediaErrorMessage",MEDIA_FULLSCREEN_UNAVAILABLE:"mediaFullscreenUnavailable",MEDIA_HAS_PLAYED:"mediaHasPlayed",MEDIA_HAS_SEEKED:"mediaHasSeeked",MEDIA_HEIGHT:"mediaHeight",MEDIA_IS_AIRPLAYING:"mediaIsAirplaying",MEDIA_IS_CASTING:"mediaIsCasting",MEDIA_IS_FULLSCREEN:"mediaIsFullscreen",MEDIA_IS_PIP:"mediaIsPip",MEDIA_LOADING:"mediaLoading",MEDIA_MUTED:"mediaMuted",MEDIA_LOOP:"mediaLoop",MEDIA_PAUSED:"mediaPaused",MEDIA_PIP_UNAVAILABLE:"mediaPipUnavailable",MEDIA_PLAYBACK_RATE:"mediaPlaybackRate",MEDIA_PREVIEW_CHAPTER:"mediaPreviewChapter",MEDIA_PREVIEW_COORDS:"mediaPreviewCoords",MEDIA_PREVIEW_IMAGE:"mediaPreviewImage",MEDIA_PREVIEW_TIME:"mediaPreviewTime",MEDIA_RENDITION_LIST:"mediaRenditionList",MEDIA_RENDITION_SELECTED:"mediaRenditionSelected",MEDIA_RENDITION_UNAVAILABLE:"mediaRenditionUnavailable",MEDIA_SEEKABLE:"mediaSeekable",MEDIA_STREAM_TYPE:"mediaStreamType",MEDIA_SUBTITLES_LIST:"mediaSubtitlesList",MEDIA_SUBTITLES_SHOWING:"mediaSubtitlesShowing",MEDIA_TARGET_LIVE_WINDOW:"mediaTargetLiveWindow",MEDIA_TIME_IS_LIVE:"mediaTimeIsLive",MEDIA_VOLUME:"mediaVolume",MEDIA_VOLUME_LEVEL:"mediaVolumeLevel",MEDIA_VOLUME_UNAVAILABLE:"mediaVolumeUnavailable",MEDIA_LANG:"mediaLang",MEDIA_WIDTH:"mediaWidth"},cn=Object.entries(er),a=cn.reduce((i,[t,e])=>(i[t]=e.toLowerCase(),i),{}),Sa={USER_INACTIVE_CHANGE:"userinactivechange",BREAKPOINTS_CHANGE:"breakpointchange",BREAKPOINTS_COMPUTED:"breakpointscomputed"},Se=cn.reduce((i,[t,e])=>(i[t]=e.toLowerCase(),i),{...Sa}),Ia=Object.entries(Se).reduce((i,[t,e])=>{let r=a[t];return r&&(i[e]=r),i},{userinactivechange:"userinactive"}),ro=Object.entries(a).reduce((i,[t,e])=>{let r=Se[t];return r&&(i[e]=r),i},{userinactive:"userinactivechange"}),W={SUBTITLES:"subtitles",CAPTIONS:"captions",DESCRIPTIONS:"descriptions",CHAPTERS:"chapters",METADATA:"metadata"},fe={DISABLED:"disabled",HIDDEN:"hidden",SHOWING:"showing"},Ma={HAVE_NOTHING:0,HAVE_METADATA:1,HAVE_CURRENT_DATA:2,HAVE_FUTURE_DATA:3,HAVE_ENOUGH_DATA:4},zt={MOUSE:"mouse",PEN:"pen",TOUCH:"touch"},Y={UNAVAILABLE:"unavailable",UNSUPPORTED:"unsupported"},j={LIVE:"live",ON_DEMAND:"on-demand",UNKNOWN:"unknown"},_a={HIGH:"high",MEDIUM:"medium",LOW:"low",OFF:"off"},oo={INLINE:"inline",FULLSCREEN:"fullscreen",PICTURE_IN_PICTURE:"picture-in-picture"};var or={};to(or,{emptyTimeRanges:()=>vn,formatAsTimePhrase:()=>Ie,formatTime:()=>X,serializeTimeRanges:()=>xa});function un(i){return i==null?void 0:i.map(La).join(" ")}function La(i){if(i){let{id:t,width:e,height:r}=i;return[t,e,r].filter(o=>o!=null).join(":")}}function mn(i){return i==null?void 0:i.map(ka).join(" ")}function ka(i){if(i){let{id:t,kind:e,language:r,label:o}=i;return[t,e,r,o].filter(n=>n!=null).join(":")}}function ot(i){return typeof i=="number"&&!Number.isNaN(i)&&Number.isFinite(i)}var ir=i=>new Promise(t=>setTimeout(t,i));var Ra={"&":"&amp;","<":"&lt;",">":"&gt;",'"':"&quot;","'":"&#39;"};function rr(i){return String(i).replace(/[&<>"']/g,t=>Ra[t])}var hn={"Start airplay":"Start airplay","Stop airplay":"Stop airplay",Audio:"Audio",Captions:"Captions","Enable captions":"Enable captions","Disable captions":"Disable captions","Start casting":"Start casting","Stop casting":"Stop casting","Enter fullscreen mode":"Enter fullscreen mode","Exit fullscreen mode":"Exit fullscreen mode",Mute:"Mute",Unmute:"Unmute",Loop:"Loop","Enter picture in picture mode":"Enter picture in picture mode","Exit picture in picture mode":"Exit picture in picture mode",Play:"Play",Pause:"Pause","Playback rate":"Playback rate","Playback rate {playbackRate}":"Playback rate {playbackRate}",Quality:"Quality","Seek backward":"Seek backward","Seek forward":"Seek forward",Settings:"Settings",Auto:"Auto","audio player":"audio player","video player":"video player",volume:"volume",seek:"seek","closed captions":"closed captions","current playback rate":"current playback rate","playback time":"playback time","media loading":"media loading",settings:"settings","audio tracks":"audio tracks",quality:"quality",play:"play",pause:"pause",mute:"mute",unmute:"unmute","chapter: {chapterName}":"chapter: {chapterName}",live:"live",Off:"Off","start airplay":"start airplay","stop airplay":"stop airplay","start casting":"start casting","stop casting":"stop casting","enter fullscreen mode":"enter fullscreen mode","exit fullscreen mode":"exit fullscreen mode","enter picture in picture mode":"enter picture in picture mode","exit picture in picture mode":"exit picture in picture mode","seek to live":"seek to live","playing live":"playing live","seek back {seekOffset} seconds":"seek back {seekOffset} seconds","seek forward {seekOffset} seconds":"seek forward {seekOffset} seconds","Network Error":"Network Error","Decode Error":"Decode Error","Source Not Supported":"Source Not Supported","Encryption Error":"Encryption Error","A network error caused the media download to fail.":"A network error caused the media download to fail.","A media error caused playback to be aborted. The media could be corrupt or your browser does not support this format.":"A media error caused playback to be aborted. The media could be corrupt or your browser does not support this format.","An unsupported error occurred. The server or network failed, or your browser does not support this format.":"An unsupported error occurred. The server or network failed, or your browser does not support this format.","The media is encrypted and there are no keys to decrypt it.":"The media is encrypted and there are no keys to decrypt it.",hour:"hour",hours:"hours",minute:"minute",minutes:"minutes",second:"second",seconds:"seconds","{time} remaining":"{time} remaining","{currentTime} of {totalTime}":"{currentTime} of {totalTime}","video not loaded, unknown time.":"video not loaded, unknown time."};var Zt={en:hn},pn,nt=((pn=globalThis.navigator)==null?void 0:pn.language)||"en",En=i=>{nt=i};var Da=i=>{var e,r,o;let[t]=nt.split("-");return((e=Zt[nt])==null?void 0:e[i])||((r=Zt[t])==null?void 0:r[i])||((o=Zt.en)==null?void 0:o[i])||i},gn=()=>{let[i]=nt.split("-");return Zt[nt]?nt:Zt[i]?i:"en"},g=(i,t={})=>Da(i).replace(/\{(\w+)\}/g,(e,r)=>r in t?String(t[r]):`{${r}}`);var bn=[{singular:"hour",plural:"hours"},{singular:"minute",plural:"minutes"},{singular:"second",plural:"seconds"}],fn=(i,t)=>{let e=i===1?g(bn[t].singular):g(bn[t].plural);return`${i} ${e}`},Ie=i=>{if(!ot(i))return"";let t=Math.abs(i),e=t!==i,n=[Math.floor(t/3600),Math.floor(t/60%60),Math.floor(t%60)].map((d,c)=>d&&fn(d,c)).filter(d=>d).join(", ")||fn(0,2);return e?g("{time} remaining",{time:n}):n};function X(i,t){let e=!1;i<0&&(e=!0,i=0-i),i=i<0?0:i;let r=Math.floor(i%60),o=Math.floor(i/60%60),n=Math.floor(i/3600),d=Math.floor(t/60%60),c=Math.floor(t/3600);return(isNaN(i)||i===1/0)&&(n=o=r="0"),n=n>0||c>0?n+":":"",o=((n||d>=10)&&o<10?"0"+o:o)+":",r=r<10?"0"+r:r,(e?"-":"")+n+o+r}var vn=Object.freeze({length:0,start(i){let t=i>>>0;if(t>=this.length)throw new DOMException(`Failed to execute 'start' on 'TimeRanges': The index provided (${t}) is greater than or equal to the maximum bound (${this.length}).`);return 0},end(i){let t=i>>>0;if(t>=this.length)throw new DOMException(`Failed to execute 'end' on 'TimeRanges': The index provided (${t}) is greater than or equal to the maximum bound (${this.length}).`);return 0}});function xa(i=vn){return Array.from(i).map((t,e)=>[Number(i.start(e).toFixed(3)),Number(i.end(e).toFixed(3))].join(":")).join(" ")}var nr=class{addEventListener(){}removeEventListener(){}dispatchEvent(){return!0}},sr=class extends nr{},ar=class extends sr{constructor(){super(...arguments);this.role=null}},no=class{observe(){}unobserve(){}disconnect(){}},Tn={createElement:function(){return new Xt.HTMLElement},createElementNS:function(){return new Xt.HTMLElement},addEventListener(){},removeEventListener(){},dispatchEvent(i){return!1}},Xt={ResizeObserver:no,document:Tn,Node:sr,Element:ar,HTMLElement:class extends ar{constructor(){super(...arguments);this.innerHTML=""}get content(){return new Xt.DocumentFragment}},DocumentFragment:class extends nr{},customElements:{get:function(){},define:function(){},whenDefined:function(){}},localStorage:{getItem(i){return null},setItem(i,t){},removeItem(i){}},CustomEvent:function(){},getComputedStyle:function(){},navigator:{languages:[],get userAgent(){return""}},matchMedia(i){return{matches:!1,media:i}},DOMParser:class{parseFromString(t,e){return{body:{textContent:t}}}}},An="global"in globalThis&&(globalThis==null?void 0:globalThis.global)===globalThis||typeof window=="undefined"||typeof window.customElements=="undefined",yn=Object.keys(Xt).every(i=>i in globalThis),l=An&&!yn?Xt:globalThis,K=An&&!yn?Tn:globalThis.document;var Sn=new WeakMap,so=i=>{let t=Sn.get(i);return t||Sn.set(i,t=new Set),t},In=new l.ResizeObserver(i=>{for(let t of i)for(let e of so(t.target))e(t)});function dr(i,t){so(i).add(t),In.observe(i)}function lr(i,t){let e=so(i);e.delete(t),e.size||In.unobserve(i)}function B(i){let t={};for(let e of i)t[e.name]=e.value;return t}function Mn(i){var t;return(t=wa(i))!=null?t:Me(i,"media-controller")}function wa(i){var r;let{MEDIA_CONTROLLER:t}=k,e=i.getAttribute(t);if(e)return(r=Oa(i))==null?void 0:r.getElementById(e)}var cr=(i,t,e=".value")=>{let r=i.querySelector(e);r&&(r.textContent=t)},Ca=(i,t)=>{let e=`slot[name="${t}"]`,r=i.shadowRoot.querySelector(e);return r?r.children:[]},ur=(i,t)=>Ca(i,t)[0],me=(i,t)=>!i||!t?!1:i!=null&&i.contains(t)?!0:me(i,t.getRootNode().host),Me=(i,t)=>{if(!i)return null;let e=i.closest(t);return e||Me(i.getRootNode().host,t)};function ao(i=document){var e;let t=i==null?void 0:i.activeElement;return t?(e=ao(t.shadowRoot))!=null?e:t:null}function Oa(i){var e;let t=(e=i==null?void 0:i.getRootNode)==null?void 0:e.call(i);return t instanceof ShadowRoot||t instanceof Document?t:null}function mr(i,{depth:t=3,checkOpacity:e=!0,checkVisibilityCSS:r=!0}={}){if(i.checkVisibility)return i.checkVisibility({checkOpacity:e,checkVisibilityCSS:r});let o=i;for(;o&&t>0;){let n=getComputedStyle(o);if(e&&n.opacity==="0"||r&&n.visibility==="hidden"||n.display==="none")return!1;o=o.parentElement,t--}return!0}function _n(i,t,e,r){let o=r.x-e.x,n=r.y-e.y,d=o*o+n*n;if(d===0)return 0;let c=((i-e.x)*o+(t-e.y)*n)/d;return Math.max(0,Math.min(1,c))}function F(i,t){let e=Pa(i,r=>r===t);return e||lo(i,t)}function Pa(i,t){var r,o;let e;for(e of(r=i.querySelectorAll("style:not([media])"))!=null?r:[]){let n;try{n=(o=e.sheet)==null?void 0:o.cssRules}catch{continue}for(let d of n!=null?n:[])if(t(d.selectorText))return d}}function lo(i,t){var n,d;let e=(n=i.querySelectorAll("style:not([media])"))!=null?n:[],r=e==null?void 0:e[e.length-1];if(!(r!=null&&r.sheet))return console.warn("Media Chrome: No style sheet found on style tag of",i),{style:{setProperty:()=>{},removeProperty:()=>"",getPropertyValue:()=>""}};let o=r==null?void 0:r.sheet.insertRule(`${t}{}`,r.sheet.cssRules.length);return(d=r.sheet.cssRules)==null?void 0:d[o]}function P(i,t,e=Number.NaN){let r=i.getAttribute(t);return r!=null?+r:e}function H(i,t,e){let r=+e;if(e==null||Number.isNaN(r)){i.hasAttribute(t)&&i.removeAttribute(t);return}P(i,t,void 0)!==r&&i.setAttribute(t,`${r}`)}function y(i,t){return i.hasAttribute(t)}function S(i,t,e){if(e==null){i.hasAttribute(t)&&i.removeAttribute(t);return}y(i,t)!=e&&i.toggleAttribute(t,e)}function D(i,t,e=null){var r;return(r=i.getAttribute(t))!=null?r:e}function R(i,t,e){if(e==null){i.hasAttribute(t)&&i.removeAttribute(t);return}let r=`${e}`;D(i,t,void 0)!==r&&i.setAttribute(t,r)}function Ua(i){return`
    <style>
      :host {
        display: var(--media-control-display, var(--media-gesture-receiver-display, inline-block));
        box-sizing: border-box;
      }
    </style>
  `}var q,st=class extends l.HTMLElement{constructor(){super();u(this,q,void 0);if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}}static get observedAttributes(){return[k.MEDIA_CONTROLLER,a.MEDIA_PAUSED]}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===k.MEDIA_CONTROLLER&&(r&&((d=(n=s(this,q))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,q,null)),o&&this.isConnected&&(h(this,q,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,q))==null?void 0:m.associateElement)==null||b.call(m,this)))}connectedCallback(){var e,r;this.tabIndex=-1,this.setAttribute("aria-hidden","true"),h(this,q,Na(this)),this.getAttribute(k.MEDIA_CONTROLLER)&&((r=(e=s(this,q))==null?void 0:e.associateElement)==null||r.call(e,this)),s(this,q)&&(s(this,q).addEventListener("pointerdown",this),s(this,q).addEventListener("click",this),s(this,q).hasAttribute("tabindex")||(s(this,q).tabIndex=0))}disconnectedCallback(){var e,r,o,n;this.getAttribute(k.MEDIA_CONTROLLER)&&((r=(e=s(this,q))==null?void 0:e.unassociateElement)==null||r.call(e,this)),(o=s(this,q))==null||o.removeEventListener("pointerdown",this),(n=s(this,q))==null||n.removeEventListener("click",this),h(this,q,null)}handleEvent(e){var n;let r=(n=e.composedPath())==null?void 0:n[0];if(["video","media-controller"].includes(r==null?void 0:r.localName)){if(e.type==="pointerdown")this._pointerType=e.pointerType;else if(e.type==="click"){let{clientX:d,clientY:c}=e,{left:m,top:b,width:A,height:M}=this.getBoundingClientRect(),T=d-m,v=c-b;if(T<0||v<0||T>A||v>M||A===0&&M===0)return;let x=this._pointerType||"mouse";if(this._pointerType=void 0,x===zt.TOUCH){this.handleTap(e);return}else if(x===zt.MOUSE||x===zt.PEN){this.handleMouseClick(e);return}}}}get mediaPaused(){return y(this,a.MEDIA_PAUSED)}set mediaPaused(e){S(this,a.MEDIA_PAUSED,e)}handleTap(e){}handleMouseClick(e){let r=this.mediaPaused?E.MEDIA_PLAY_REQUEST:E.MEDIA_PAUSE_REQUEST;this.dispatchEvent(new l.CustomEvent(r,{composed:!0,bubbles:!0}))}};q=new WeakMap,st.shadowRootOptions={mode:"open"},st.getTemplateHTML=Ua;function Na(i){var e;let t=i.getAttribute(k.MEDIA_CONTROLLER);return t?(e=i.getRootNode())==null?void 0:e.getElementById(t):Me(i,"media-controller")}l.customElements.get("media-gesture-receiver")||l.customElements.define("media-gesture-receiver",st);var Jt=st;var I={AUDIO:"audio",AUTOHIDE:"autohide",BREAKPOINTS:"breakpoints",GESTURES_DISABLED:"gesturesdisabled",KEYBOARD_CONTROL:"keyboardcontrol",NO_AUTOHIDE:"noautohide",USER_INACTIVE:"userinactive",AUTOHIDE_OVER_CONTROLS:"autohideovercontrols"};function Ha(i){return`
    <style>
      
      :host([${a.MEDIA_IS_FULLSCREEN}]) ::slotted([slot=media]) {
        outline: none;
      }

      :host {
        box-sizing: border-box;
        position: relative;
        display: inline-block;
        line-height: 0;
        background-color: var(--media-background-color, #000);
        overflow: hidden;
      }

      :host(:not([${I.AUDIO}])) [part~=layer]:not([part~=media-layer]) {
        position: absolute;
        top: 0;
        left: 0;
        bottom: 0;
        right: 0;
        display: flex;
        flex-flow: column nowrap;
        align-items: start;
        pointer-events: none;
        background: none;
      }

      slot[name=media] {
        display: var(--media-slot-display, contents);
      }

      
      :host([${I.AUDIO}]) slot[name=media] {
        display: var(--media-slot-display, none);
      }

      
      :host([${I.AUDIO}]) [part~=layer][part~=gesture-layer] {
        height: 0;
        display: block;
      }

      
      :host(:not([${I.AUDIO}])[${I.GESTURES_DISABLED}]) ::slotted([slot=gestures-chrome]),
          :host(:not([${I.AUDIO}])[${I.GESTURES_DISABLED}]) media-gesture-receiver[slot=gestures-chrome] {
        display: none;
      }

      
      ::slotted(:not([slot=media]):not([slot=poster]):not(media-loading-indicator):not([role=dialog]):not([hidden])) {
        pointer-events: auto;
      }

      :host(:not([${I.AUDIO}])) *[part~=layer][part~=centered-layer] {
        align-items: center;
        justify-content: center;
      }

      :host(:not([${I.AUDIO}])) ::slotted(media-gesture-receiver[slot=gestures-chrome]),
      :host(:not([${I.AUDIO}])) media-gesture-receiver[slot=gestures-chrome] {
        align-self: stretch;
        flex-grow: 1;
      }

      slot[name=middle-chrome] {
        display: inline;
        flex-grow: 1;
        pointer-events: none;
        background: none;
      }

      
      ::slotted([slot=media]),
      ::slotted([slot=poster]) {
        width: 100%;
        height: 100%;
      }

      
      :host(:not([${I.AUDIO}])) .spacer {
        flex-grow: 1;
      }

      
      :host(:-webkit-full-screen) {
        
        width: 100% !important;
        height: 100% !important;
      }

      
      ::slotted(:not([slot=media]):not([slot=poster]):not([${I.NO_AUTOHIDE}]):not([hidden]):not([role=dialog])) {
        opacity: 1;
        transition: var(--media-control-transition-in, opacity 0.25s);
      }

      
      :host([${I.USER_INACTIVE}]:not([${a.MEDIA_PAUSED}]):not([${a.MEDIA_IS_AIRPLAYING}]):not([${a.MEDIA_IS_CASTING}]):not([${I.AUDIO}])) ::slotted(:not([slot=media]):not([slot=poster]):not([${I.NO_AUTOHIDE}]):not([role=dialog])) {
        opacity: 0;
        transition: var(--media-control-transition-out, opacity 1s);
      }

      :host([${I.USER_INACTIVE}]:not([${I.NO_AUTOHIDE}]):not([${a.MEDIA_PAUSED}]):not([${a.MEDIA_IS_CASTING}]):not([${I.AUDIO}])) ::slotted([slot=media]) {
        cursor: none;
      }

      :host([${I.USER_INACTIVE}][${I.AUTOHIDE_OVER_CONTROLS}]:not([${I.NO_AUTOHIDE}]):not([${a.MEDIA_PAUSED}]):not([${a.MEDIA_IS_CASTING}]):not([${I.AUDIO}])) * {
        --media-cursor: none;
        cursor: none;
      }


      ::slotted(media-control-bar)  {
        align-self: stretch;
      }

      
      
      :host(:not([${I.AUDIO}])[${a.MEDIA_HAS_PLAYED}]) slot[name=poster],
      :host(:not([${I.AUDIO}])[${a.MEDIA_STREAM_TYPE}="${j.ON_DEMAND}"][${a.MEDIA_HAS_SEEKED}]) slot[name=poster] {
        display: none;
      }

      ::slotted([role=dialog]) {
        width: 100%;
        height: 100%;
        align-self: center;
      }

      ::slotted([role=menu]) {
        align-self: end;
      }
    </style>

    <slot name="media" part="layer media-layer"></slot>
    <slot name="poster" part="layer poster-layer"></slot>
    <slot name="gestures-chrome" part="layer gesture-layer">
      <media-gesture-receiver slot="gestures-chrome">
        <template shadowrootmode="${Jt.shadowRootOptions.mode}">
          ${Jt.getTemplateHTML({})}
        </template>
      </media-gesture-receiver>
    </slot>
    <span part="layer vertical-layer">
      <slot name="top-chrome" part="top chrome"></slot>
      <slot name="middle-chrome" part="middle chrome"></slot>
      <slot name="centered-chrome" part="layer centered-layer center centered chrome"></slot>
      
      <slot part="bottom chrome"></slot>
    </span>
    <slot name="dialog" part="layer dialog-layer"></slot>
  `}var Fa=Object.values(a),$a="sm:384 md:576 lg:768 xl:960";function Ba(i){Ln(i.target,i.contentRect.width)}function Ln(i,t){var d;if(!i.isConnected)return;let e=(d=i.getAttribute(I.BREAKPOINTS))!=null?d:$a,r=Va(e),o=Ka(r,t),n=!1;if(Object.keys(r).forEach(c=>{if(o.includes(c)){i.hasAttribute(`breakpoint${c}`)||(i.setAttribute(`breakpoint${c}`,""),n=!0);return}i.hasAttribute(`breakpoint${c}`)&&(i.removeAttribute(`breakpoint${c}`),n=!0)}),n){let c=new CustomEvent(Se.BREAKPOINTS_CHANGE,{detail:o});i.dispatchEvent(c)}i.breakpointsComputed||(i.breakpointsComputed=!0,i.dispatchEvent(new CustomEvent(Se.BREAKPOINTS_COMPUTED,{bubbles:!0,composed:!0})))}function Va(i){let t=i.split(/\s+/);return Object.fromEntries(t.map(e=>e.split(":")))}function Ka(i,t){return Object.keys(i).filter(e=>t>=parseInt(i[e]))}var at,ti,Pe,Ue,Ne,pr,He,ii,Er,kn,gr,Rn,dt,hr,ri,co,Fe,ei,ve,oi,_e=class extends l.HTMLElement{constructor(){super();u(this,Er);u(this,gr);u(this,dt);u(this,ri);u(this,Fe);u(this,at,void 0);u(this,ti,0);u(this,Pe,null);u(this,Ue,null);u(this,Ne,void 0);this.breakpointsComputed=!1;u(this,pr,e=>{let r=this.media;for(let o of e){if(o.type!=="childList")continue;let n=o.removedNodes;for(let d of n){if(d.slot!="media"||o.target!=this)continue;let c=o.previousSibling&&o.previousSibling.previousElementSibling;if(!c||!r)this.mediaUnsetCallback(d);else{let m=c.slot!=="media";for(;(c=c.previousSibling)!==null;)c.slot=="media"&&(m=!1);m&&this.mediaUnsetCallback(d)}}if(r)for(let d of o.addedNodes)d===r&&this.handleMediaUpdated(r)}});u(this,He,!1);u(this,ii,e=>{s(this,He)||(setTimeout(()=>{Ba(e),h(this,He,!1)},0),h(this,He,!0))});u(this,ve,void 0);u(this,oi,()=>{if(!s(this,ve).assignedElements({flatten:!0}).length){s(this,Pe)&&this.mediaUnsetCallback(s(this,Pe));return}this.handleMediaUpdated(this.media)});if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes),r=this.constructor.getTemplateHTML(e);this.shadowRoot.setHTMLUnsafe?this.shadowRoot.setHTMLUnsafe(r):this.shadowRoot.innerHTML=r}h(this,at,new MutationObserver(s(this,pr)))}static get observedAttributes(){return[I.AUTOHIDE,I.GESTURES_DISABLED].concat(Fa).filter(e=>![a.MEDIA_RENDITION_LIST,a.MEDIA_AUDIO_TRACK_LIST,a.MEDIA_CHAPTERS_CUES,a.MEDIA_WIDTH,a.MEDIA_HEIGHT,a.MEDIA_ERROR,a.MEDIA_ERROR_MESSAGE].includes(e))}attributeChangedCallback(e,r,o){e.toLowerCase()==I.AUTOHIDE&&(this.autohide=o)}get media(){let e=this.querySelector(":scope > [slot=media]");return(e==null?void 0:e.nodeName)=="SLOT"&&(e=e.assignedElements({flatten:!0})[0]),e}async handleMediaUpdated(e){e&&(h(this,Pe,e),e.localName.includes("-")&&await l.customElements.whenDefined(e.localName),this.mediaSetCallback(e))}connectedCallback(){var n;s(this,at).observe(this,{childList:!0,subtree:!0}),dr(this,s(this,ii));let r=this.getAttribute(I.AUDIO)!=null?g("audio player"):g("video player");this.setAttribute("role","region"),this.setAttribute("aria-label",r),this.handleMediaUpdated(this.media),this.setAttribute(I.USER_INACTIVE,""),Ln(this,this.getBoundingClientRect().width);let o=this.querySelector(":scope > slot[slot=media]");o&&(h(this,ve,o),s(this,ve).addEventListener("slotchange",s(this,oi))),this.addEventListener("pointerdown",this),this.addEventListener("pointermove",this),this.addEventListener("pointerup",this),this.addEventListener("mouseleave",this),this.addEventListener("keyup",this),(n=l.window)==null||n.addEventListener("mouseup",this)}disconnectedCallback(){var e;lr(this,s(this,ii)),clearTimeout(s(this,Ue)),s(this,at).disconnect(),this.media&&this.mediaUnsetCallback(this.media),(e=l.window)==null||e.removeEventListener("mouseup",this),this.removeEventListener("pointerdown",this),this.removeEventListener("pointermove",this),this.removeEventListener("pointerup",this),this.removeEventListener("mouseleave",this),this.removeEventListener("keyup",this),s(this,ve)&&(s(this,ve).removeEventListener("slotchange",s(this,oi)),h(this,ve,null)),h(this,He,!1)}mediaSetCallback(e){}mediaUnsetCallback(e){h(this,Pe,null)}handleEvent(e){switch(e.type){case"pointerdown":h(this,ti,e.timeStamp);break;case"pointermove":f(this,Er,kn).call(this,e);break;case"pointerup":f(this,gr,Rn).call(this,e);break;case"mouseleave":f(this,dt,hr).call(this);break;case"mouseup":this.removeAttribute(I.KEYBOARD_CONTROL);break;case"keyup":f(this,Fe,ei).call(this),this.setAttribute(I.KEYBOARD_CONTROL,"");break}}set autohide(e){let r=Number(e);h(this,Ne,isNaN(r)?0:r)}get autohide(){return(s(this,Ne)===void 0?2:s(this,Ne)).toString()}get breakpoints(){return D(this,I.BREAKPOINTS)}set breakpoints(e){R(this,I.BREAKPOINTS,e)}get audio(){return y(this,I.AUDIO)}set audio(e){S(this,I.AUDIO,e)}get gesturesDisabled(){return y(this,I.GESTURES_DISABLED)}set gesturesDisabled(e){S(this,I.GESTURES_DISABLED,e)}get keyboardControl(){return y(this,I.KEYBOARD_CONTROL)}set keyboardControl(e){S(this,I.KEYBOARD_CONTROL,e)}get noAutohide(){return y(this,I.NO_AUTOHIDE)}set noAutohide(e){S(this,I.NO_AUTOHIDE,e)}get autohideOverControls(){return y(this,I.AUTOHIDE_OVER_CONTROLS)}set autohideOverControls(e){S(this,I.AUTOHIDE_OVER_CONTROLS,e)}get userInteractive(){return y(this,I.USER_INACTIVE)}set userInteractive(e){S(this,I.USER_INACTIVE,e)}};at=new WeakMap,ti=new WeakMap,Pe=new WeakMap,Ue=new WeakMap,Ne=new WeakMap,pr=new WeakMap,He=new WeakMap,ii=new WeakMap,Er=new WeakSet,kn=function(e){if(e.pointerType!=="mouse"&&e.timeStamp-s(this,ti)<250)return;f(this,ri,co).call(this),clearTimeout(s(this,Ue));let r=this.hasAttribute(I.AUTOHIDE_OVER_CONTROLS);([this,this.media].includes(e.target)||r)&&f(this,Fe,ei).call(this)},gr=new WeakSet,Rn=function(e){if(e.pointerType==="touch"){let r=!this.hasAttribute(I.USER_INACTIVE);[this,this.media].includes(e.target)&&r?f(this,dt,hr).call(this):f(this,Fe,ei).call(this)}else e.composedPath().some(r=>["media-play-button","media-fullscreen-button"].includes(r==null?void 0:r.localName))&&f(this,Fe,ei).call(this)},dt=new WeakSet,hr=function(){if(s(this,Ne)<0||this.hasAttribute(I.USER_INACTIVE))return;this.setAttribute(I.USER_INACTIVE,"");let e=new l.CustomEvent(Se.USER_INACTIVE_CHANGE,{composed:!0,bubbles:!0,detail:!0});this.dispatchEvent(e)},ri=new WeakSet,co=function(){if(!this.hasAttribute(I.USER_INACTIVE))return;this.removeAttribute(I.USER_INACTIVE);let e=new l.CustomEvent(Se.USER_INACTIVE_CHANGE,{composed:!0,bubbles:!0,detail:!1});this.dispatchEvent(e)},Fe=new WeakSet,ei=function(){f(this,ri,co).call(this),clearTimeout(s(this,Ue));let e=parseInt(this.autohide);e<0||h(this,Ue,setTimeout(()=>{f(this,dt,hr).call(this)},e*1e3))},ve=new WeakMap,oi=new WeakMap,_e.shadowRootOptions={mode:"open"},_e.getTemplateHTML=Ha;l.customElements.get("media-container")||l.customElements.define("media-container",_e);var Dn=_e;function Ga(i){return`
    <style>
      :host {
        font: var(--media-font,
          var(--media-font-weight, normal)
          var(--media-font-size, 14px) /
          var(--media-text-content-height, var(--media-control-height, 24px))
          var(--media-font-family, helvetica neue, segoe ui, roboto, arial, sans-serif));
        color: var(--media-text-color, var(--media-primary-color, rgb(238 238 238)));
        display: var(--media-dialog-display, inline-flex);
        justify-content: center;
        align-items: center;
        
        transition-behavior: allow-discrete;
        visibility: hidden;
        opacity: 0;
        transform: translateY(2px) scale(.99);
        pointer-events: none;
      }

      :host([open]) {
        transition: display .2s, visibility 0s, opacity .2s ease-out, transform .15s ease-out;
        visibility: visible;
        opacity: 1;
        transform: translateY(0) scale(1);
        pointer-events: auto;
      }

      #content {
        display: flex;
        position: relative;
        box-sizing: border-box;
        width: min(320px, 100%);
        word-wrap: break-word;
        max-height: 100%;
        overflow: auto;
        text-align: center;
        line-height: 1.4;
      }
    </style>
    ${this.getSlotTemplateHTML(i)}
  `}function qa(i){return`
    <slot id="content"></slot>
  `}var ni={OPEN:"open",ANCHOR:"anchor"},si,lt,Le,ai,uo,br,xn,fr,wn,vr,Cn,Tr,On,Ar,Pn,ie=class extends l.HTMLElement{constructor(){super();u(this,ai);u(this,br);u(this,fr);u(this,vr);u(this,Tr);u(this,Ar);u(this,si,!1);u(this,lt,null);u(this,Le,null)}static get observedAttributes(){return[ni.OPEN,ni.ANCHOR]}get open(){return y(this,ni.OPEN)}set open(e){S(this,ni.OPEN,e)}handleEvent(e){switch(e.type){case"invoke":f(this,vr,Cn).call(this,e);break;case"focusout":f(this,Tr,On).call(this,e);break;case"keydown":f(this,Ar,Pn).call(this,e);break}}connectedCallback(){f(this,ai,uo).call(this),this.role||(this.role="dialog"),this.addEventListener("invoke",this),this.addEventListener("focusout",this),this.addEventListener("keydown",this)}disconnectedCallback(){this.removeEventListener("invoke",this),this.removeEventListener("focusout",this),this.removeEventListener("keydown",this)}attributeChangedCallback(e,r,o){f(this,ai,uo).call(this),e===ni.OPEN&&o!==r&&(this.open?f(this,br,xn).call(this):f(this,fr,wn).call(this))}focus(){h(this,lt,ao());let e=!this.dispatchEvent(new Event("focus",{composed:!0,cancelable:!0})),r=!this.dispatchEvent(new Event("focusin",{composed:!0,bubbles:!0,cancelable:!0}));if(e||r)return;let o=this.querySelector('[autofocus], [tabindex]:not([tabindex="-1"]), [role="menu"]');o==null||o.focus()}get keysUsed(){return["Escape","Tab"]}};si=new WeakMap,lt=new WeakMap,Le=new WeakMap,ai=new WeakSet,uo=function(){if(!s(this,si)&&(h(this,si,!0),!this.shadowRoot)){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e),queueMicrotask(()=>{let{style:r}=F(this.shadowRoot,":host");r.setProperty("transition","display .15s, visibility .15s, opacity .15s ease-in, transform .15s ease-in")})}},br=new WeakSet,xn=function(){var e;(e=s(this,Le))==null||e.setAttribute("aria-expanded","true"),this.dispatchEvent(new Event("open",{composed:!0,bubbles:!0})),this.addEventListener("transitionend",()=>this.focus(),{once:!0})},fr=new WeakSet,wn=function(){var e;(e=s(this,Le))==null||e.setAttribute("aria-expanded","false"),this.dispatchEvent(new Event("close",{composed:!0,bubbles:!0}))},vr=new WeakSet,Cn=function(e){h(this,Le,e.relatedTarget),me(this,e.relatedTarget)||(this.open=!this.open)},Tr=new WeakSet,On=function(e){var r;me(this,e.relatedTarget)||((r=s(this,lt))==null||r.focus(),s(this,Le)&&s(this,Le)!==e.relatedTarget&&this.open&&(this.open=!1))},Ar=new WeakSet,Pn=function(e){var c,m,b,A,M;let{key:r,ctrlKey:o,altKey:n,metaKey:d}=e;o||n||d||this.keysUsed.includes(r)&&(e.preventDefault(),e.stopPropagation(),r==="Tab"?(e.shiftKey?(m=(c=this.previousElementSibling)==null?void 0:c.focus)==null||m.call(c):(A=(b=this.nextElementSibling)==null?void 0:b.focus)==null||A.call(b),this.blur()):r==="Escape"&&((M=s(this,lt))==null||M.focus(),this.open=!1))},ie.shadowRootOptions={mode:"open"},ie.getTemplateHTML=Ga,ie.getSlotTemplateHTML=qa;l.customElements.get("media-chrome-dialog")||l.customElements.define("media-chrome-dialog",ie);var Un=ie;var di={HOTKEYS:"hotkeys"},Wa=[{keys:[{key:" ",label:"Space"},{key:"k"}],description:"Toggle Playback"},{keys:[{key:"m"}],description:"Toggle mute"},{keys:[{key:"f"}],description:"Toggle fullscreen"},{keys:[{key:"c"}],description:"Toggle captions or subtitles, if available"},{keys:[{key:"p"}],description:"Toggle Picture in Picture"},{keys:[{key:"ArrowLeft",label:"\u2190"},{key:"j"}],description:"Seek back 10s"},{keys:[{key:"ArrowRight",label:"\u2192"},{key:"l"}],description:"Seek forward 10s"},{keys:[{key:"ArrowUp",label:"\u2191"}],description:"Turn volume up"},{keys:[{key:"ArrowDown",label:"\u2193"}],description:"Turn volume down"},{keys:[{key:"<",label:"< (SHIFT+,)"}],description:"Decrease playback rate"},{keys:[{key:">",label:"> (SHIFT+.)"}],description:"Increase playback rate"},{keys:[{key:"shift+/",label:"SHIFT+/"}],description:"Show keyboard shortcuts"}];function yr(i){return i===" "?"nospace":`no${i.toLowerCase()}`}function Ya(i){let t=new Set(i==null?void 0:i.split(/\s+/));return Wa.map(({keys:e,description:r})=>({keys:e.filter(({key:o})=>!t.has(yr(o))),description:r})).filter(({keys:e})=>e.length)}function Qa(i){return`
    <style>
      :host {
        position: fixed;
        top: 0;
        left: 0;
        z-index: 9999;
        background: rgb(20 20 30 / .8);
        backdrop-filter: blur(10px);
      }

      #content {
        display: block;
        width: clamp(400px, 40vw, 700px);
        max-width: 90vw;
        text-align: left;
      }

      h2 {
        margin: 0 0 1.5rem 0;
        font-size: 1.5rem;
        font-weight: 500;
        text-align: center;
      }

      .shortcuts-table {
        width: 100%;
        border-collapse: collapse;
      }

      .shortcuts-table tr {
        border-bottom: 1px solid rgba(255, 255, 255, 0.1);
      }

      .shortcuts-table tr:last-child {
        border-bottom: none;
      }

      .shortcuts-table td {
        padding: 0.75rem 0.5rem;
      }

      .shortcuts-table td:first-child {
        text-align: right;
        padding-right: 1rem;
        width: 40%;
        min-width: 120px;
      }

      .shortcuts-table td:last-child {
        padding-left: 1rem;
      }

      .key {
        display: inline-block;
        background: rgba(255, 255, 255, 0.15);
        border: 1px solid rgba(255, 255, 255, 0.2);
        border-radius: 4px;
        padding: 0.25rem 0.5rem;
        font-family: 'Courier New', monospace;
        font-size: 0.9rem;
        font-weight: 500;
        min-width: 1.5rem;
        text-align: center;
        margin: 0 0.2rem;
      }

      .description {
        color: rgba(255, 255, 255, 0.9);
        font-size: 0.95rem;
      }

      .key-combo {
        display: flex;
        align-items: center;
        justify-content: flex-end;
        gap: 0.3rem;
      }

      .key-separator {
        color: rgba(255, 255, 255, 0.5);
        font-size: 0.9rem;
      }
    </style>
    <slot id="content">
      ${Nn(i[di.HOTKEYS])}
    </slot>
  `}function Nn(i){return`
    <h2>Keyboard Shortcuts</h2>
    <table class="shortcuts-table">${Ya(i).map(({keys:e,description:r})=>`
        <tr>
          <td>
            <div class="key-combo">${e.map(({key:n,label:d=n},c)=>c>0?`<span class="key-separator">or</span><span class="key">${d}</span>`:`<span class="key">${d}</span>`).join("")}</div>
          </td>
          <td class="description">${r}</td>
        </tr>
      `).join("")}</table>
  `}var $e,Be,li=class extends ie{constructor(){super(...arguments);u(this,$e,e=>{var c;if(!this.open)return;let r=(c=this.shadowRoot)==null?void 0:c.querySelector("#content");if(!r)return;let o=e.composedPath(),n=o[0]===this||o.includes(this),d=o.includes(r);n&&!d&&(this.open=!1)});u(this,Be,e=>{if(!this.open)return;let r=e.shiftKey&&(e.key==="/"||e.key==="?");(e.key==="Escape"||r)&&!e.ctrlKey&&!e.altKey&&!e.metaKey&&(this.open=!1,e.preventDefault(),e.stopPropagation())})}static get observedAttributes(){return[...super.observedAttributes,di.HOTKEYS]}get hotkeys(){var e;return(e=D(this,di.HOTKEYS))!=null?e:void 0}set hotkeys(e){R(this,di.HOTKEYS,e)}connectedCallback(){super.connectedCallback(),this.open&&(this.addEventListener("click",s(this,$e)),document.addEventListener("keydown",s(this,Be)))}disconnectedCallback(){this.removeEventListener("click",s(this,$e)),document.removeEventListener("keydown",s(this,Be))}attributeChangedCallback(e,r,o){var n;if(super.attributeChangedCallback(e,r,o),e==="open")this.open?(this.addEventListener("click",s(this,$e)),document.addEventListener("keydown",s(this,Be))):(this.removeEventListener("click",s(this,$e)),document.removeEventListener("keydown",s(this,Be)));else if(e===di.HOTKEYS&&o!==r){let d=(n=this.shadowRoot)==null?void 0:n.querySelector("#content");d&&(d.innerHTML=Nn(o))}}};$e=new WeakMap,Be=new WeakMap,li.getSlotTemplateHTML=Qa;l.customElements.get("media-keyboard-shortcuts-dialog")||l.customElements.define("media-keyboard-shortcuts-dialog",li);var Hn=li;var Ve,Ke,ci,Re,he,ke,ct=class{constructor(t,e,{defaultValue:r}={defaultValue:void 0}){u(this,he);u(this,Ve,void 0);u(this,Ke,void 0);u(this,ci,void 0);u(this,Re,new Set);h(this,Ve,t),h(this,Ke,e),h(this,ci,new Set(r))}[Symbol.iterator](){return s(this,he,ke).values()}get length(){return s(this,he,ke).size}get value(){var t;return(t=[...s(this,he,ke)].join(" "))!=null?t:""}set value(t){var e;t!==this.value&&(h(this,Re,new Set),this.add(...(e=t==null?void 0:t.split(" "))!=null?e:[]))}toString(){return this.value}item(t){return[...s(this,he,ke)][t]}values(){return s(this,he,ke).values()}forEach(t,e){s(this,he,ke).forEach(t,e)}add(...t){var e,r;t.forEach(o=>s(this,Re).add(o)),!(this.value===""&&!((e=s(this,Ve))!=null&&e.hasAttribute(`${s(this,Ke)}`)))&&((r=s(this,Ve))==null||r.setAttribute(`${s(this,Ke)}`,`${this.value}`))}remove(...t){var e;t.forEach(r=>s(this,Re).delete(r)),(e=s(this,Ve))==null||e.setAttribute(`${s(this,Ke)}`,`${this.value}`)}contains(t){return s(this,he,ke).has(t)}toggle(t,e){return typeof e!="undefined"?e?(this.add(t),!0):(this.remove(t),!1):this.contains(t)?(this.remove(t),!1):(this.add(t),!0)}replace(t,e){return this.remove(t),this.add(e),t===e}};Ve=new WeakMap,Ke=new WeakMap,ci=new WeakMap,Re=new WeakMap,he=new WeakSet,ke=function(){return s(this,Re).size?s(this,Re):s(this,ci)};var ja=(i="")=>i.split(/\s+/),Fn=(i="")=>{let[t,e,r]=i.split(":"),o=r!==void 0?decodeURIComponent(r):void 0;return{kind:t==="cc"?W.CAPTIONS:W.SUBTITLES,language:e,label:o}},mo=(i="",t={})=>ja(i).map(e=>{let r=Fn(e);return{...t,...r}}),ho=i=>i?Array.isArray(i)?i.map(t=>typeof t=="string"?Fn(t):t):typeof i=="string"?mo(i):[i]:[],za=({kind:i,label:t,language:e}={kind:"subtitles"})=>`${i==="captions"?"cc":"sb"}:${e}:${encodeURIComponent(t!=null?t:"")}`,ui=(i=[])=>Array.prototype.map.call(i,za).join(" "),Za=(i,t)=>e=>e[i]===t,$n=i=>{let t=Object.entries(i).map(([e,r])=>Za(e,r));return e=>t.every(r=>r(e))},Ge=(i,t=[],e=[])=>{let r=ho(e).map($n),o=n=>r.some(d=>d(n));Array.from(t).filter(o).forEach(n=>{n.mode=i})},qe=(i,t=()=>!0)=>{if(!(i!=null&&i.textTracks))return[];let e=typeof t=="function"?t:$n(t);return Array.from(i.textTracks).filter(e)},Bn=i=>{var e;return!!((e=i.mediaSubtitlesShowing)!=null&&e.length)||i.hasAttribute(a.MEDIA_SUBTITLES_SHOWING)};var Kn=i=>{var r;let{media:t,fullscreenElement:e}=i;try{let o=e&&"requestFullscreen"in e?"requestFullscreen":e&&"webkitRequestFullScreen"in e?"webkitRequestFullScreen":void 0;if(o){let n=(r=e[o])==null?void 0:r.call(e);if(n instanceof Promise)return n.catch(()=>{})}else t!=null&&t.webkitEnterFullscreen?t.webkitEnterFullscreen():t!=null&&t.requestFullscreen&&t.requestFullscreen()}catch(o){console.error(o)}},Vn="exitFullscreen"in K?"exitFullscreen":"webkitExitFullscreen"in K?"webkitExitFullscreen":"webkitCancelFullScreen"in K?"webkitCancelFullScreen":void 0,Gn=i=>{var e;let{documentElement:t}=i;if(Vn){let r=(e=t==null?void 0:t[Vn])==null?void 0:e.call(t);if(r instanceof Promise)return r.catch(()=>{})}},mi="fullscreenElement"in K?"fullscreenElement":"webkitFullscreenElement"in K?"webkitFullscreenElement":void 0,Xa=i=>{let{documentElement:t,media:e}=i,r=t==null?void 0:t[mi];return!r&&"webkitDisplayingFullscreen"in e&&"webkitPresentationMode"in e&&e.webkitDisplayingFullscreen&&e.webkitPresentationMode===oo.FULLSCREEN?e:r},qn=i=>{var n;let{media:t,documentElement:e,fullscreenElement:r=t}=i;if(!t||!e)return!1;let o=Xa(i);if(!o)return!1;if(o===r||o===t)return!0;if(o.localName.includes("-")){let d=o.shadowRoot;if(!(mi in d))return me(o,r);for(;d!=null&&d[mi];){if(d[mi]===r)return!0;d=(n=d[mi])==null?void 0:n.shadowRoot}}return!1},Ja="fullscreenEnabled"in K?"fullscreenEnabled":"webkitFullscreenEnabled"in K?"webkitFullscreenEnabled":void 0,Wn=i=>{let{documentElement:t,media:e}=i;return!!(t!=null&&t[Ja])||e&&"webkitSupportsFullscreen"in e};var Sr,po=()=>{var i,t;return Sr||(Sr=(t=(i=K)==null?void 0:i.createElement)==null?void 0:t.call(i,"video"),Sr)},Yn=async(i=po())=>{if(!i)return!1;let t=i.volume;i.volume=t/2+.1;let e=new AbortController,r=await Promise.race([ed(i,e.signal),td(i,t)]);return e.abort(),r},ed=(i,t)=>new Promise(e=>{i.addEventListener("volumechange",()=>e(!0),{signal:t})}),td=async(i,t)=>{for(let e=0;e<10;e++){if(i.volume===t)return!1;await ir(10)}return i.volume!==t},id=/.*Version\/.*Safari\/.*/.test(l.navigator.userAgent),Eo=(i=po())=>l.matchMedia("(display-mode: standalone)").matches&&id?!1:typeof(i==null?void 0:i.requestPictureInPicture)=="function",go=(i=po())=>Wn({documentElement:K,media:i}),Qn=go(),jn=Eo(),zn=!!l.WebKitPlaybackTargetAvailabilityEvent,Zn=!!l.chrome;var ut=i=>qe(i.media,t=>[W.SUBTITLES,W.CAPTIONS].includes(t.kind)).sort((t,e)=>t.kind>=e.kind?1:-1),bo=i=>qe(i.media,t=>t.mode===fe.SHOWING&&[W.SUBTITLES,W.CAPTIONS].includes(t.kind)),Ir=(i,t)=>{let e=ut(i),r=bo(i),o=!!r.length;if(e.length){if(t===!1||o&&t!==!0)Ge(fe.DISABLED,e,r);else if(t===!0||!o&&t!==!1){let n=e[0],{options:d}=i;if(!(d!=null&&d.noSubtitlesLangPref)){let A=l.localStorage.getItem("media-chrome-pref-subtitles-lang"),M=A?[A,...l.navigator.languages]:l.navigator.languages,T=e.filter(v=>M.some(x=>v.language.toLowerCase().startsWith(x.split("-")[0]))).sort((v,x)=>{let _=M.findIndex(C=>v.language.toLowerCase().startsWith(C.split("-")[0])),L=M.findIndex(C=>x.language.toLowerCase().startsWith(C.split("-")[0]));return _-L});T[0]&&(n=T[0])}let{language:c,label:m,kind:b}=n;Ge(fe.DISABLED,e,r),Ge(fe.SHOWING,e,[{language:c,label:m,kind:b}])}}},Mr=(i,t)=>i===t?!0:i==null||t==null||typeof i!=typeof t?!1:typeof i=="number"&&Number.isNaN(i)&&Number.isNaN(t)?!0:typeof i!="object"?!1:Array.isArray(i)?rd(i,t):Object.entries(i).every(([e,r])=>e in t&&Mr(r,t[e])),rd=(i,t)=>{let e=Array.isArray(i),r=Array.isArray(t);return e!==r?!1:e||r?i.length!==t.length?!1:i.every((o,n)=>Mr(o,t[n])):!0};var od=Object.values(j),_r,nd=Yn().then(i=>(_r=i,_r)),Xn=async(...i)=>{await Promise.all(i.filter(t=>t).map(async t=>{if(!("localName"in t&&t instanceof l.HTMLElement))return;let e=t.localName;if(!e.includes("-"))return;let r=l.customElements.get(e);r&&t instanceof r||(await l.customElements.whenDefined(e),l.customElements.upgrade(t))}))},sd=new l.DOMParser,ad=i=>i&&(sd.parseFromString(i,"text/html").body.textContent||i),mt={mediaError:{get(i,t){let{media:e}=i;if((t==null?void 0:t.type)!=="playing")return e==null?void 0:e.error},mediaEvents:["emptied","error","playing"]},mediaErrorCode:{get(i,t){var r;let{media:e}=i;if((t==null?void 0:t.type)!=="playing")return(r=e==null?void 0:e.error)==null?void 0:r.code},mediaEvents:["emptied","error","playing"]},mediaErrorMessage:{get(i,t){var r,o;let{media:e}=i;if((t==null?void 0:t.type)!=="playing")return(o=(r=e==null?void 0:e.error)==null?void 0:r.message)!=null?o:""},mediaEvents:["emptied","error","playing"]},mediaWidth:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.videoWidth)!=null?e:0},mediaEvents:["resize"]},mediaHeight:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.videoHeight)!=null?e:0},mediaEvents:["resize"]},mediaPaused:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.paused)!=null?e:!0},set(i,t){var r;let{media:e}=t;e&&(i?e.pause():(r=e.play())==null||r.catch(()=>{}))},mediaEvents:["play","playing","pause","emptied"]},mediaHasPlayed:{get(i,t){let{media:e}=i;return e?t?t.type==="playing":!e.paused:!1},mediaEvents:["playing","emptied"]},mediaHasSeeked:{get(i,t){let{media:e}=i;return e?t?t.type==="seeking":!!e.seeking:!1},mediaEvents:["seeking","emptied"]},mediaEnded:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.ended)!=null?e:!1},mediaEvents:["seeked","ended","emptied"]},mediaPlaybackRate:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.playbackRate)!=null?e:1},set(i,t){let{media:e}=t;e&&Number.isFinite(+i)&&(e.playbackRate=+i)},mediaEvents:["ratechange","loadstart"]},mediaMuted:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.muted)!=null?e:!1},set(i,t){let{media:e,options:{noMutedPref:r}={}}=t;if(e){e.muted=i;try{let o=l.localStorage.getItem("media-chrome-pref-muted")!==null,n=e.hasAttribute("muted");if(r){o&&l.localStorage.removeItem("media-chrome-pref-muted");return}if(n&&!o)return;l.localStorage.setItem("media-chrome-pref-muted",i?"true":"false")}catch(o){console.debug("Error setting muted pref",o)}}},mediaEvents:["volumechange"],stateOwnersUpdateHandlers:[(i,t)=>{let{options:{noMutedPref:e}}=t,{media:r}=t;if(!(!r||r.muted||e))try{let o=l.localStorage.getItem("media-chrome-pref-muted")==="true";mt.mediaMuted.set(o,t),i(o)}catch(o){console.debug("Error getting muted pref",o)}}]},mediaLoop:{get(i){let{media:t}=i;return t==null?void 0:t.loop},set(i,t){let{media:e}=t;e&&(e.loop=i)},mediaEvents:["medialooprequest"]},mediaVolume:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.volume)!=null?e:1},set(i,t){let{media:e,options:{noVolumePref:r}={}}=t;if(e){try{i==null?l.localStorage.removeItem("media-chrome-pref-volume"):!e.hasAttribute("muted")&&!r&&l.localStorage.setItem("media-chrome-pref-volume",i.toString())}catch(o){console.debug("Error setting volume pref",o)}Number.isFinite(+i)&&(e.volume=+i)}},mediaEvents:["volumechange"],stateOwnersUpdateHandlers:[(i,t)=>{let{options:{noVolumePref:e}}=t;if(!e)try{let{media:r}=t;if(!r)return;let o=l.localStorage.getItem("media-chrome-pref-volume");if(o==null)return;mt.mediaVolume.set(+o,t),i(+o)}catch(r){console.debug("Error getting volume pref",r)}}]},mediaVolumeLevel:{get(i){let{media:t}=i;return typeof(t==null?void 0:t.volume)=="undefined"?"high":t.muted||t.volume===0?"off":t.volume<.5?"low":t.volume<.75?"medium":"high"},mediaEvents:["volumechange"]},mediaCurrentTime:{get(i){var e;let{media:t}=i;return(e=t==null?void 0:t.currentTime)!=null?e:0},set(i,t){let{media:e}=t;!e||!ot(i)||(e.currentTime=i)},mediaEvents:["timeupdate","loadedmetadata","seeking"]},mediaDuration:{get(i){let{media:t,options:{defaultDuration:e}={}}=i;return e&&(!t||!t.duration||Number.isNaN(t.duration)||!Number.isFinite(t.duration))?e:Number.isFinite(t==null?void 0:t.duration)?t.duration:Number.NaN},mediaEvents:["durationchange","loadedmetadata","emptied"]},mediaLoading:{get(i){let{media:t}=i;return(t==null?void 0:t.readyState)<3},mediaEvents:["waiting","playing","emptied"]},mediaSeekable:{get(i){var o;let{media:t}=i;if(!((o=t==null?void 0:t.seekable)!=null&&o.length))return;let e=t.seekable.start(0),r=t.seekable.end(t.seekable.length-1);if(!(!e&&!r))return[Number(e.toFixed(3)),Number(r.toFixed(3))]},mediaEvents:["loadedmetadata","emptied","progress","seekablechange"]},mediaBuffered:{get(i){var r;let{media:t}=i,e=(r=t==null?void 0:t.buffered)!=null?r:[];return Array.from(e).map((o,n)=>[Number(e.start(n).toFixed(3)),Number(e.end(n).toFixed(3))])},mediaEvents:["progress","emptied"]},mediaStreamType:{get(i){let{media:t,options:{defaultStreamType:e}={}}=i,r=[j.LIVE,j.ON_DEMAND].includes(e)?e:void 0;if(!t)return r;let{streamType:o}=t;if(od.includes(o))return o===j.UNKNOWN?r:o;let n=t.duration;return n===1/0?j.LIVE:Number.isFinite(n)?j.ON_DEMAND:r},mediaEvents:["emptied","durationchange","loadedmetadata","streamtypechange"]},mediaTargetLiveWindow:{get(i){let{media:t}=i;if(!t)return Number.NaN;let{targetLiveWindow:e}=t,r=mt.mediaStreamType.get(i);return(e==null||Number.isNaN(e))&&r===j.LIVE?0:e},mediaEvents:["emptied","durationchange","loadedmetadata","streamtypechange","targetlivewindowchange"]},mediaTimeIsLive:{get(i){let{media:t,options:{liveEdgeOffset:e=10}={}}=i;if(!t)return!1;if(typeof t.liveEdgeStart=="number")return Number.isNaN(t.liveEdgeStart)?!1:t.currentTime>=t.liveEdgeStart;if(!(mt.mediaStreamType.get(i)===j.LIVE))return!1;let o=t.seekable;if(!o)return!0;if(!o.length)return!1;let n=o.end(o.length-1)-e;return t.currentTime>=n},mediaEvents:["playing","timeupdate","progress","waiting","emptied"]},mediaSubtitlesList:{get(i){return ut(i).map(({kind:t,label:e,language:r})=>({kind:t,label:e,language:r}))},mediaEvents:["loadstart"],textTracksEvents:["addtrack","removetrack"]},mediaSubtitlesShowing:{get(i){return bo(i).map(({kind:t,label:e,language:r})=>({kind:t,label:e,language:r}))},mediaEvents:["loadstart"],textTracksEvents:["addtrack","removetrack","change"],stateOwnersUpdateHandlers:[(i,t)=>{var n,d;let{media:e,options:r}=t;if(!e)return;let o=c=>{var b;!r.defaultSubtitles||c&&![W.CAPTIONS,W.SUBTITLES].includes((b=c==null?void 0:c.track)==null?void 0:b.kind)||Ir(t,!0)};return e.addEventListener("loadstart",o),(n=e.textTracks)==null||n.addEventListener("addtrack",o),(d=e.textTracks)==null||d.addEventListener("removetrack",o),()=>{var c,m;e.removeEventListener("loadstart",o),(c=e.textTracks)==null||c.removeEventListener("addtrack",o),(m=e.textTracks)==null||m.removeEventListener("removetrack",o)}}]},mediaChaptersCues:{get(i){var r;let{media:t}=i;if(!t)return[];let[e]=qe(t,{kind:W.CHAPTERS});return Array.from((r=e==null?void 0:e.cues)!=null?r:[]).map(({text:o,startTime:n,endTime:d})=>({text:ad(o),startTime:n,endTime:d}))},mediaEvents:["loadstart","loadedmetadata"],textTracksEvents:["addtrack","removetrack","change"],stateOwnersUpdateHandlers:[(i,t)=>{var n;let{media:e}=t;if(!e)return;let r=e.querySelector('track[kind="chapters"][default][src]'),o=(n=e.shadowRoot)==null?void 0:n.querySelector(':is(video,audio) > track[kind="chapters"][default][src]');return r==null||r.addEventListener("load",i),o==null||o.addEventListener("load",i),()=>{r==null||r.removeEventListener("load",i),o==null||o.removeEventListener("load",i)}}]},mediaIsPip:{get(i){var r,o;let{media:t,documentElement:e}=i;if(!t||!e||!e.pictureInPictureElement)return!1;if(e.pictureInPictureElement===t)return!0;if(e.pictureInPictureElement instanceof HTMLMediaElement)return(r=t.localName)!=null&&r.includes("-")?me(t,e.pictureInPictureElement):!1;if(e.pictureInPictureElement.localName.includes("-")){let n=e.pictureInPictureElement.shadowRoot;for(;n!=null&&n.pictureInPictureElement;){if(n.pictureInPictureElement===t)return!0;n=(o=n.pictureInPictureElement)==null?void 0:o.shadowRoot}}return!1},set(i,t){let{media:e}=t;if(e)if(i){if(!K.pictureInPictureEnabled){console.warn("MediaChrome: Picture-in-picture is not enabled");return}if(!e.requestPictureInPicture){console.warn("MediaChrome: The current media does not support picture-in-picture");return}let r=()=>{console.warn("MediaChrome: The media is not ready for picture-in-picture. It must have a readyState > 0.")};e.requestPictureInPicture().catch(o=>{if(o.code===11){if(!e.src){console.warn("MediaChrome: The media is not ready for picture-in-picture. It must have a src set.");return}if(e.readyState===0&&e.preload==="none"){let n=()=>{e.removeEventListener("loadedmetadata",d),e.preload="none"},d=()=>{e.requestPictureInPicture().catch(r),n()};e.addEventListener("loadedmetadata",d),e.preload="metadata",setTimeout(()=>{e.readyState===0&&r(),n()},1e3)}else throw o}else throw o})}else K.pictureInPictureElement&&K.exitPictureInPicture()},mediaEvents:["enterpictureinpicture","leavepictureinpicture"]},mediaRenditionList:{get(i){var e;let{media:t}=i;return[...(e=t==null?void 0:t.videoRenditions)!=null?e:[]].map(r=>({...r}))},mediaEvents:["emptied","loadstart"],videoRenditionsEvents:["addrendition","removerendition"]},mediaRenditionSelected:{get(i){var e,r,o;let{media:t}=i;return(o=(r=t==null?void 0:t.videoRenditions)==null?void 0:r[(e=t.videoRenditions)==null?void 0:e.selectedIndex])==null?void 0:o.id},set(i,t){let{media:e}=t;if(!(e!=null&&e.videoRenditions)){console.warn("MediaController: Rendition selection not supported by this media.");return}let r=i,o=Array.prototype.findIndex.call(e.videoRenditions,n=>n.id==r);e.videoRenditions.selectedIndex!=o&&(e.videoRenditions.selectedIndex=o)},mediaEvents:["emptied"],videoRenditionsEvents:["addrendition","removerendition","change"]},mediaAudioTrackList:{get(i){var e;let{media:t}=i;return[...(e=t==null?void 0:t.audioTracks)!=null?e:[]]},mediaEvents:["emptied","loadstart"],audioTracksEvents:["addtrack","removetrack"]},mediaAudioTrackEnabled:{get(i){var e,r;let{media:t}=i;return(r=[...(e=t==null?void 0:t.audioTracks)!=null?e:[]].find(o=>o.enabled))==null?void 0:r.id},set(i,t){let{media:e}=t;if(!(e!=null&&e.audioTracks)){console.warn("MediaChrome: Audio track selection not supported by this media.");return}let r=i;for(let o of e.audioTracks)o.enabled=r==o.id},mediaEvents:["emptied"],audioTracksEvents:["addtrack","removetrack","change"]},mediaIsFullscreen:{get(i){return qn(i)},set(i,t,e){var r,o;i?(Kn(t),e.detail&&!((r=t.media)!=null&&r.inert)&&((o=t.media)==null||o.focus())):Gn(t)},rootEvents:["fullscreenchange","webkitfullscreenchange"],mediaEvents:["webkitbeginfullscreen","webkitendfullscreen","webkitpresentationmodechanged"]},mediaIsCasting:{get(i){var e;let{media:t}=i;return!(t!=null&&t.remote)||((e=t.remote)==null?void 0:e.state)==="disconnected"?!1:t.remote.state==="connected"},set(i,t){var r,o;let{media:e}=t;if(e&&!(i&&((r=e.remote)==null?void 0:r.state)!=="disconnected")&&!(!i&&((o=e.remote)==null?void 0:o.state)!=="connected")){if(typeof e.remote.prompt!="function"){console.warn("MediaChrome: Casting is not supported in this environment");return}e.remote.prompt().catch(()=>{})}},remoteEvents:["connect","connecting","disconnect"]},mediaIsAirplaying:{get(){return!1},set(i,t){let{media:e}=t;if(e){if(!(e.webkitShowPlaybackTargetPicker&&l.WebKitPlaybackTargetAvailabilityEvent)){console.error("MediaChrome: received a request to select AirPlay but AirPlay is not supported in this environment");return}e.webkitShowPlaybackTargetPicker()}},mediaEvents:["webkitcurrentplaybacktargetiswirelesschanged"]},mediaFullscreenUnavailable:{get(i){let{media:t}=i;if(!Qn||!go(t))return Y.UNSUPPORTED}},mediaPipUnavailable:{get(i){let{media:t}=i;if(!jn||!Eo(t))return Y.UNSUPPORTED;if(t!=null&&t.disablePictureInPicture)return Y.UNAVAILABLE}},mediaVolumeUnavailable:{get(i){let{media:t}=i;if(_r===!1||(t==null?void 0:t.volume)==null)return Y.UNSUPPORTED},stateOwnersUpdateHandlers:[i=>{_r==null&&nd.then(t=>i(t?void 0:Y.UNSUPPORTED))}]},mediaCastUnavailable:{get(i,{availability:t="not-available"}={}){var r;let{media:e}=i;if(!Zn||!((r=e==null?void 0:e.remote)!=null&&r.state))return Y.UNSUPPORTED;if(!(t==null||t==="available"))return Y.UNAVAILABLE},stateOwnersUpdateHandlers:[(i,t)=>{var o;let{media:e}=t;return e?(e.disableRemotePlayback||e.hasAttribute("disableremoteplayback")||(o=e==null?void 0:e.remote)==null||o.watchAvailability(n=>{i({availability:n?"available":"not-available"})}).catch(n=>{n.name==="NotSupportedError"?i({availability:null}):i({availability:"not-available"})}),()=>{var n;(n=e==null?void 0:e.remote)==null||n.cancelWatchAvailability().catch(()=>{})}):void 0}]},mediaAirplayUnavailable:{get(i,t){if(!zn)return Y.UNSUPPORTED;if((t==null?void 0:t.availability)==="not-available")return Y.UNAVAILABLE},mediaEvents:["webkitplaybacktargetavailabilitychanged"],stateOwnersUpdateHandlers:[(i,t)=>{var o;let{media:e}=t;return e?(e.disableRemotePlayback||e.hasAttribute("disableremoteplayback")||(o=e==null?void 0:e.remote)==null||o.watchAvailability(n=>{i({availability:n?"available":"not-available"})}).catch(n=>{n.name==="NotSupportedError"?i({availability:null}):i({availability:"not-available"})}),()=>{var n;(n=e==null?void 0:e.remote)==null||n.cancelWatchAvailability().catch(()=>{})}):void 0}]},mediaRenditionUnavailable:{get(i){var e;let{media:t}=i;if(!(t!=null&&t.videoRenditions))return Y.UNSUPPORTED;if(!((e=t.videoRenditions)!=null&&e.length))return Y.UNAVAILABLE},mediaEvents:["emptied","loadstart"],videoRenditionsEvents:["addrendition","removerendition"]},mediaAudioTrackUnavailable:{get(i){var e,r;let{media:t}=i;if(!(t!=null&&t.audioTracks))return Y.UNSUPPORTED;if(((r=(e=t.audioTracks)==null?void 0:e.length)!=null?r:0)<=1)return Y.UNAVAILABLE},mediaEvents:["emptied","loadstart"],audioTracksEvents:["addtrack","removetrack"]},mediaLang:{get(i){let{options:{mediaLang:t}={}}=i;return t!=null?t:"en"}}};var Jn={[E.MEDIA_PREVIEW_REQUEST](i,t,{detail:e}){var A,M,T;let{media:r}=t,o=e!=null?e:void 0,n,d;if(r&&o!=null){let[v]=qe(r,{kind:W.METADATA,label:"thumbnails"}),x=Array.prototype.find.call((A=v==null?void 0:v.cues)!=null?A:[],(_,L,C)=>L===0?_.endTime>o:L===C.length-1?_.startTime<=o:_.startTime<=o&&_.endTime>o);if(x){let _=/'^(?:[a-z]+:)?\/\//i.test(x.text)||(M=r==null?void 0:r.querySelector('track[label="thumbnails"]'))==null?void 0:M.src,L=new URL(x.text,_);d=new URLSearchParams(L.hash).get("#xywh").split(",").map(O=>+O),n=L.href}}let c=i.mediaDuration.get(t),b=(T=i.mediaChaptersCues.get(t).find((v,x,_)=>x===_.length-1&&c===v.endTime?v.startTime<=o&&v.endTime>=o:v.startTime<=o&&v.endTime>o))==null?void 0:T.text;return e!=null&&b==null&&(b=""),{mediaPreviewTime:o,mediaPreviewImage:n,mediaPreviewCoords:d,mediaPreviewChapter:b}},[E.MEDIA_PAUSE_REQUEST](i,t){i["mediaPaused"].set(!0,t)},[E.MEDIA_PLAY_REQUEST](i,t){var c,m,b,A;let e="mediaPaused",o=i.mediaStreamType.get(t)===j.LIVE,n=!((c=t.options)!=null&&c.noAutoSeekToLive),d=i.mediaTargetLiveWindow.get(t)>0;if(o&&n&&!d){let M=(m=i.mediaSeekable.get(t))==null?void 0:m[1];if(M){let T=(A=(b=t.options)==null?void 0:b.seekToLiveOffset)!=null?A:0,v=M-T;i.mediaCurrentTime.set(v,t)}}i[e].set(!1,t)},[E.MEDIA_PLAYBACK_RATE_REQUEST](i,t,{detail:e}){let r="mediaPlaybackRate",o=e;i[r].set(o,t)},[E.MEDIA_MUTE_REQUEST](i,t){i["mediaMuted"].set(!0,t)},[E.MEDIA_UNMUTE_REQUEST](i,t){let e="mediaMuted";i.mediaVolume.get(t)||i.mediaVolume.set(.25,t),i[e].set(!1,t)},[E.MEDIA_LOOP_REQUEST](i,t,{detail:e}){let r="mediaLoop",o=!!e;return i[r].set(o,t),{mediaLoop:o}},[E.MEDIA_VOLUME_REQUEST](i,t,{detail:e}){let r="mediaVolume",o=e;o&&i.mediaMuted.get(t)&&i.mediaMuted.set(!1,t),i[r].set(o,t)},[E.MEDIA_SEEK_REQUEST](i,t,{detail:e}){let r="mediaCurrentTime",o=e;i[r].set(o,t)},[E.MEDIA_SEEK_TO_LIVE_REQUEST](i,t){var d,c,m;let e="mediaCurrentTime",r=(d=i.mediaSeekable.get(t))==null?void 0:d[1];if(Number.isNaN(Number(r)))return;let o=(m=(c=t.options)==null?void 0:c.seekToLiveOffset)!=null?m:0,n=r-o;i[e].set(n,t)},[E.MEDIA_SHOW_SUBTITLES_REQUEST](i,t,{detail:e}){var c;let{options:r}=t,o=ut(t),n=ho(e),d=(c=n[0])==null?void 0:c.language;d&&!r.noSubtitlesLangPref&&l.localStorage.setItem("media-chrome-pref-subtitles-lang",d),Ge(fe.SHOWING,o,n)},[E.MEDIA_DISABLE_SUBTITLES_REQUEST](i,t,{detail:e}){let r=ut(t),o=e!=null?e:[];Ge(fe.DISABLED,r,o)},[E.MEDIA_TOGGLE_SUBTITLES_REQUEST](i,t,{detail:e}){Ir(t,e)},[E.MEDIA_RENDITION_REQUEST](i,t,{detail:e}){let r="mediaRenditionSelected",o=e;i[r].set(o,t)},[E.MEDIA_AUDIO_TRACK_REQUEST](i,t,{detail:e}){let r="mediaAudioTrackEnabled",o=e;i[r].set(o,t)},[E.MEDIA_ENTER_PIP_REQUEST](i,t){let e="mediaIsPip";i.mediaIsFullscreen.get(t)&&i.mediaIsFullscreen.set(!1,t),i[e].set(!0,t)},[E.MEDIA_EXIT_PIP_REQUEST](i,t){i["mediaIsPip"].set(!1,t)},[E.MEDIA_ENTER_FULLSCREEN_REQUEST](i,t,e){let r="mediaIsFullscreen";i.mediaIsPip.get(t)&&i.mediaIsPip.set(!1,t),i[r].set(!0,t,e)},[E.MEDIA_EXIT_FULLSCREEN_REQUEST](i,t){i["mediaIsFullscreen"].set(!1,t)},[E.MEDIA_ENTER_CAST_REQUEST](i,t){let e="mediaIsCasting";i.mediaIsFullscreen.get(t)&&i.mediaIsFullscreen.set(!1,t),i[e].set(!0,t)},[E.MEDIA_EXIT_CAST_REQUEST](i,t){i["mediaIsCasting"].set(!1,t)},[E.MEDIA_AIRPLAY_REQUEST](i,t){i["mediaIsAirplaying"].set(!0,t)}};var es=({media:i,fullscreenElement:t,documentElement:e,stateMediator:r=mt,requestMap:o=Jn,options:n={},monitorStateOwnersOnlyWithSubscriptions:d=!0})=>{let c=[],m={options:{...n}},b=Object.freeze({mediaPreviewTime:void 0,mediaPreviewImage:void 0,mediaPreviewCoords:void 0,mediaPreviewChapter:void 0}),A=_=>{_!=null&&(Mr(_,b)||(b=Object.freeze({...b,..._}),c.forEach(L=>L(b))))},M=()=>{let _=Object.entries(r).reduce((L,[C,{get:O}])=>(L[C]=O(m),L),{});A(_)},T={},v,x=async(_,L)=>{var Wo,Yo,Qo,jo,zo,Zo,Xo,Jo,en,tn,rn,on,nn,sn,an,dn;let C=!!v;if(v={...m,...v!=null?v:{},..._},C)return;await Xn(...Object.values(_));let O=c.length>0&&L===0&&d,ee=m.media!==v.media,Xr=((Wo=m.media)==null?void 0:Wo.textTracks)!==((Yo=v.media)==null?void 0:Yo.textTracks),Yt=((Qo=m.media)==null?void 0:Qo.videoRenditions)!==((jo=v.media)==null?void 0:jo.videoRenditions),it=((zo=m.media)==null?void 0:zo.audioTracks)!==((Zo=v.media)==null?void 0:Zo.audioTracks),ge=((Xo=m.media)==null?void 0:Xo.remote)!==((Jo=v.media)==null?void 0:Jo.remote),be=m.documentElement!==v.documentElement,Qt=!!m.media&&(ee||O),Oe=!!((en=m.media)!=null&&en.textTracks)&&(Xr||O),Po=!!((tn=m.media)!=null&&tn.videoRenditions)&&(Yt||O),Uo=!!((rn=m.media)!=null&&rn.audioTracks)&&(it||O),No=!!((on=m.media)!=null&&on.remote)&&(ge||O),Ho=!!m.documentElement&&(be||O),Jr=Qt||Oe||Po||Uo||No||Ho,rt=c.length===0&&L===1&&d,Fo=!!v.media&&(ee||rt),$o=!!((nn=v.media)!=null&&nn.textTracks)&&(Xr||rt),Bo=!!((sn=v.media)!=null&&sn.videoRenditions)&&(Yt||rt),Vo=!!((an=v.media)!=null&&an.audioTracks)&&(it||rt),Ko=!!((dn=v.media)!=null&&dn.remote)&&(ge||rt),Go=!!v.documentElement&&(be||rt),qo=Fo||$o||Bo||Vo||Ko||Go;if(!(Jr||qo)){Object.entries(v).forEach(([N,jt])=>{m[N]=jt}),M(),v=void 0;return}Object.entries(r).forEach(([N,{get:jt,mediaEvents:ua=[],textTracksEvents:ma=[],videoRenditionsEvents:ha=[],audioTracksEvents:pa=[],remoteEvents:Ea=[],rootEvents:ga=[],stateOwnersUpdateHandlers:ba=[]}])=>{T[N]||(T[N]={});let Z=$=>{let V=jt(m,$);A({[N]:V})},G;G=T[N].mediaEvents,ua.forEach($=>{G&&Qt&&(m.media.removeEventListener($,G),T[N].mediaEvents=void 0),Fo&&(v.media.addEventListener($,Z),T[N].mediaEvents=Z)}),G=T[N].textTracksEvents,ma.forEach($=>{var V,te;G&&Oe&&((V=m.media.textTracks)==null||V.removeEventListener($,G),T[N].textTracksEvents=void 0),$o&&((te=v.media.textTracks)==null||te.addEventListener($,Z),T[N].textTracksEvents=Z)}),G=T[N].videoRenditionsEvents,ha.forEach($=>{var V,te;G&&Po&&((V=m.media.videoRenditions)==null||V.removeEventListener($,G),T[N].videoRenditionsEvents=void 0),Bo&&((te=v.media.videoRenditions)==null||te.addEventListener($,Z),T[N].videoRenditionsEvents=Z)}),G=T[N].audioTracksEvents,pa.forEach($=>{var V,te;G&&Uo&&((V=m.media.audioTracks)==null||V.removeEventListener($,G),T[N].audioTracksEvents=void 0),Vo&&((te=v.media.audioTracks)==null||te.addEventListener($,Z),T[N].audioTracksEvents=Z)}),G=T[N].remoteEvents,Ea.forEach($=>{var V,te;G&&No&&((V=m.media.remote)==null||V.removeEventListener($,G),T[N].remoteEvents=void 0),Ko&&((te=v.media.remote)==null||te.addEventListener($,Z),T[N].remoteEvents=Z)}),G=T[N].rootEvents,ga.forEach($=>{G&&Ho&&(m.documentElement.removeEventListener($,G),T[N].rootEvents=void 0),Go&&(v.documentElement.addEventListener($,Z),T[N].rootEvents=Z)});let Ji=T[N].stateOwnersUpdateHandlers;if(Ji&&Jr&&(Array.isArray(Ji)?Ji:[Ji]).forEach(V=>{typeof V=="function"&&V()}),qo){let $=ba.map(V=>V(Z,v)).filter(V=>typeof V=="function");T[N].stateOwnersUpdateHandlers=$.length===1?$[0]:$}else Jr&&(T[N].stateOwnersUpdateHandlers=void 0)}),Object.entries(v).forEach(([N,jt])=>{m[N]=jt}),M(),v=void 0};return x({media:i,fullscreenElement:t,documentElement:e,options:n}),{dispatch(_){let{type:L,detail:C}=_;if(o[L]&&b.mediaErrorCode==null){A(o[L](r,m,_));return}L==="mediaelementchangerequest"?x({media:C}):L==="fullscreenelementchangerequest"?x({fullscreenElement:C}):L==="documentelementchangerequest"?x({documentElement:C}):L==="optionschangerequest"&&(Object.entries(C!=null?C:{}).forEach(([O,ee])=>{m.options[O]=ee}),M())},getState(){return b},subscribe(_){return x({},c.length+1),c.push(_),_(b),()=>{let L=c.indexOf(_);L>=0&&(x({},c.length-1),c.splice(L,1))}}}};var ts=["ArrowLeft","ArrowRight","ArrowUp","ArrowDown","Enter"," ","f","m","k","c","l","j",">","<","p"],is=10,rs=.025,os=.25,dd=.25,ld=2,p={DEFAULT_SUBTITLES:"defaultsubtitles",DEFAULT_STREAM_TYPE:"defaultstreamtype",DEFAULT_DURATION:"defaultduration",FULLSCREEN_ELEMENT:"fullscreenelement",HOTKEYS:"hotkeys",KEYBOARD_BACKWARD_SEEK_OFFSET:"keyboardbackwardseekoffset",KEYBOARD_FORWARD_SEEK_OFFSET:"keyboardforwardseekoffset",KEYBOARD_DOWN_VOLUME_STEP:"keyboarddownvolumestep",KEYBOARD_UP_VOLUME_STEP:"keyboardupvolumestep",KEYS_USED:"keysused",LANG:"lang",LOOP:"loop",LIVE_EDGE_OFFSET:"liveedgeoffset",NO_AUTO_SEEK_TO_LIVE:"noautoseektolive",NO_DEFAULT_STORE:"nodefaultstore",NO_HOTKEYS:"nohotkeys",NO_MUTED_PREF:"nomutedpref",NO_SUBTITLES_LANG_PREF:"nosubtitleslangpref",NO_VOLUME_PREF:"novolumepref",SEEK_TO_LIVE_OFFSET:"seektoliveoffset"},pe,ht,w,z,pt,re,hi,Et,pi,vo,De,Ei,To,gt,Lr,Rr,as,kr=class extends _e{constructor(){super();u(this,pi);u(this,Ei);u(this,gt);u(this,Rr);this.mediaStateReceivers=[];this.associatedElementSubscriptions=new Map;u(this,pe,new ct(this,p.HOTKEYS));u(this,ht,void 0);u(this,w,void 0);u(this,z,null);u(this,pt,void 0);u(this,re,void 0);u(this,hi,e=>{var r;(r=s(this,w))==null||r.dispatch(e)});u(this,Et,void 0);u(this,De,e=>{let{key:r,shiftKey:o}=e;if(!(o&&(r==="/"||r==="?")||ts.includes(r))){this.removeEventListener("keyup",s(this,De));return}this.keyboardShortcutHandler(e)});this.associateElement(this);let e={};h(this,pt,r=>{Object.entries(r).forEach(([o,n])=>{if(o in e&&e[o]===n)return;this.propagateMediaState(o,n);let d=o.toLowerCase(),c=new l.CustomEvent(ro[d],{composed:!0,detail:n});this.dispatchEvent(c)}),e=r})}static get observedAttributes(){return super.observedAttributes.concat(p.NO_HOTKEYS,p.HOTKEYS,p.DEFAULT_STREAM_TYPE,p.DEFAULT_SUBTITLES,p.DEFAULT_DURATION,p.NO_MUTED_PREF,p.NO_VOLUME_PREF,p.LANG,p.LOOP,p.LIVE_EDGE_OFFSET,p.SEEK_TO_LIVE_OFFSET,p.NO_AUTO_SEEK_TO_LIVE)}get mediaStore(){return s(this,w)}set mediaStore(e){var r,o;if(s(this,w)&&((r=s(this,re))==null||r.call(this),h(this,re,void 0)),h(this,w,e),!s(this,w)&&!this.hasAttribute(p.NO_DEFAULT_STORE)){f(this,pi,vo).call(this);return}h(this,re,(o=s(this,w))==null?void 0:o.subscribe(s(this,pt)))}get fullscreenElement(){var e;return(e=s(this,ht))!=null?e:this}set fullscreenElement(e){var r;this.hasAttribute(p.FULLSCREEN_ELEMENT)&&this.removeAttribute(p.FULLSCREEN_ELEMENT),h(this,ht,e),(r=s(this,w))==null||r.dispatch({type:"fullscreenelementchangerequest",detail:this.fullscreenElement})}get defaultSubtitles(){return y(this,p.DEFAULT_SUBTITLES)}set defaultSubtitles(e){S(this,p.DEFAULT_SUBTITLES,e)}get defaultStreamType(){return D(this,p.DEFAULT_STREAM_TYPE)}set defaultStreamType(e){R(this,p.DEFAULT_STREAM_TYPE,e)}get defaultDuration(){return P(this,p.DEFAULT_DURATION)}set defaultDuration(e){H(this,p.DEFAULT_DURATION,e)}get noHotkeys(){return y(this,p.NO_HOTKEYS)}set noHotkeys(e){S(this,p.NO_HOTKEYS,e)}get keysUsed(){return D(this,p.KEYS_USED)}set keysUsed(e){R(this,p.KEYS_USED,e)}get liveEdgeOffset(){return P(this,p.LIVE_EDGE_OFFSET)}set liveEdgeOffset(e){H(this,p.LIVE_EDGE_OFFSET,e)}get noAutoSeekToLive(){return y(this,p.NO_AUTO_SEEK_TO_LIVE)}set noAutoSeekToLive(e){S(this,p.NO_AUTO_SEEK_TO_LIVE,e)}get noVolumePref(){return y(this,p.NO_VOLUME_PREF)}set noVolumePref(e){S(this,p.NO_VOLUME_PREF,e)}get noMutedPref(){return y(this,p.NO_MUTED_PREF)}set noMutedPref(e){S(this,p.NO_MUTED_PREF,e)}get noSubtitlesLangPref(){return y(this,p.NO_SUBTITLES_LANG_PREF)}set noSubtitlesLangPref(e){S(this,p.NO_SUBTITLES_LANG_PREF,e)}get noDefaultStore(){return y(this,p.NO_DEFAULT_STORE)}set noDefaultStore(e){S(this,p.NO_DEFAULT_STORE,e)}get resolvedLang(){return gn()}attributeChangedCallback(e,r,o){var n,d,c,m,b,A,M,T,v,x,_,L;if(super.attributeChangedCallback(e,r,o),e===p.NO_HOTKEYS)o!==r&&o===""?(this.hasAttribute(p.HOTKEYS)&&console.warn("Media Chrome: Both `hotkeys` and `nohotkeys` have been set. All hotkeys will be disabled."),this.disableHotkeys()):o!==r&&o===null&&this.enableHotkeys(),f(this,gt,Lr).call(this);else if(e===p.HOTKEYS)s(this,pe).value=o,f(this,gt,Lr).call(this);else if(e===p.DEFAULT_SUBTITLES&&o!==r)(n=s(this,w))==null||n.dispatch({type:"optionschangerequest",detail:{defaultSubtitles:this.hasAttribute(p.DEFAULT_SUBTITLES)}});else if(e===p.DEFAULT_STREAM_TYPE)(c=s(this,w))==null||c.dispatch({type:"optionschangerequest",detail:{defaultStreamType:(d=this.getAttribute(p.DEFAULT_STREAM_TYPE))!=null?d:void 0}});else if(e===p.LIVE_EDGE_OFFSET&&o!==r)(m=s(this,w))==null||m.dispatch({type:"optionschangerequest",detail:{liveEdgeOffset:this.hasAttribute(p.LIVE_EDGE_OFFSET)?+this.getAttribute(p.LIVE_EDGE_OFFSET):void 0,seekToLiveOffset:this.hasAttribute(p.SEEK_TO_LIVE_OFFSET)?+this.getAttribute(p.SEEK_TO_LIVE_OFFSET):this.hasAttribute(p.LIVE_EDGE_OFFSET)?+this.getAttribute(p.LIVE_EDGE_OFFSET):void 0}});else if(e===p.SEEK_TO_LIVE_OFFSET&&o!==r)(b=s(this,w))==null||b.dispatch({type:"optionschangerequest",detail:{seekToLiveOffset:this.hasAttribute(p.SEEK_TO_LIVE_OFFSET)?+this.getAttribute(p.SEEK_TO_LIVE_OFFSET):this.hasAttribute(p.LIVE_EDGE_OFFSET)?+this.getAttribute(p.LIVE_EDGE_OFFSET):void 0}});else if(e===p.NO_AUTO_SEEK_TO_LIVE)(A=s(this,w))==null||A.dispatch({type:"optionschangerequest",detail:{noAutoSeekToLive:this.hasAttribute(p.NO_AUTO_SEEK_TO_LIVE)}});else if(e===p.FULLSCREEN_ELEMENT){let C=o?(M=this.getRootNode())==null?void 0:M.getElementById(o):void 0;h(this,ht,C),(T=s(this,w))==null||T.dispatch({type:"fullscreenelementchangerequest",detail:this.fullscreenElement})}else e===p.LANG&&o!==r?(En(o),(v=s(this,w))==null||v.dispatch({type:"optionschangerequest",detail:{mediaLang:o}})):e===p.LOOP&&o!==r?(x=s(this,w))==null||x.dispatch({type:E.MEDIA_LOOP_REQUEST,detail:o!=null}):e===p.NO_VOLUME_PREF&&o!==r?(_=s(this,w))==null||_.dispatch({type:"optionschangerequest",detail:{noVolumePref:this.hasAttribute(p.NO_VOLUME_PREF)}}):e===p.NO_MUTED_PREF&&o!==r&&((L=s(this,w))==null||L.dispatch({type:"optionschangerequest",detail:{noMutedPref:this.hasAttribute(p.NO_MUTED_PREF)}}))}connectedCallback(){var e,r,o;this.associateElement(this),!s(this,w)&&!this.hasAttribute(p.NO_DEFAULT_STORE)&&f(this,pi,vo).call(this),(e=s(this,w))==null||e.dispatch({type:"documentelementchangerequest",detail:K}),(r=s(this,w))==null||r.dispatch({type:"fullscreenelementchangerequest",detail:this.fullscreenElement}),super.connectedCallback(),s(this,w)&&!s(this,re)&&h(this,re,(o=s(this,w))==null?void 0:o.subscribe(s(this,pt))),s(this,Et)!==void 0&&s(this,w)&&this.media&&setTimeout(()=>{var n,d,c;(d=(n=this.media)==null?void 0:n.textTracks)!=null&&d.length&&((c=s(this,w))==null||c.dispatch({type:E.MEDIA_TOGGLE_SUBTITLES_REQUEST,detail:s(this,Et)}))},0),this.hasAttribute(p.NO_HOTKEYS)?this.disableHotkeys():this.enableHotkeys()}disconnectedCallback(){var e,r,o,n,d,c;if((e=super.disconnectedCallback)==null||e.call(this),this.disableHotkeys(),s(this,w)){let m=s(this,w).getState();h(this,Et,!!((r=m.mediaSubtitlesShowing)!=null&&r.length)),(o=s(this,w))==null||o.dispatch({type:"fullscreenelementchangerequest",detail:void 0}),(n=s(this,w))==null||n.dispatch({type:"documentelementchangerequest",detail:void 0}),(d=s(this,w))==null||d.dispatch({type:E.MEDIA_TOGGLE_SUBTITLES_REQUEST,detail:!1})}s(this,re)&&((c=s(this,re))==null||c.call(this),h(this,re,void 0)),this.unassociateElement(this),s(this,z)&&(s(this,z).remove(),h(this,z,null))}mediaSetCallback(e){var r;super.mediaSetCallback(e),(r=s(this,w))==null||r.dispatch({type:"mediaelementchangerequest",detail:e}),e.hasAttribute("tabindex")||(e.tabIndex=-1)}mediaUnsetCallback(e){var r;super.mediaUnsetCallback(e),(r=s(this,w))==null||r.dispatch({type:"mediaelementchangerequest",detail:void 0})}propagateMediaState(e,r){ss(this.mediaStateReceivers,e,r)}associateElement(e){if(!e)return;let{associatedElementSubscriptions:r}=this;if(r.has(e))return;let o=this.registerMediaStateReceiver.bind(this),n=this.unregisterMediaStateReceiver.bind(this),d=Ed(e,o,n);Object.values(E).forEach(c=>{e.addEventListener(c,s(this,hi))}),r.set(e,d)}unassociateElement(e){if(!e)return;let{associatedElementSubscriptions:r}=this;if(!r.has(e))return;r.get(e)(),r.delete(e),Object.values(E).forEach(n=>{e.removeEventListener(n,s(this,hi))})}registerMediaStateReceiver(e){if(!e)return;let r=this.mediaStateReceivers;r.indexOf(e)>-1||(r.push(e),s(this,w)&&Object.entries(s(this,w).getState()).forEach(([n,d])=>{ss([e],n,d)}))}unregisterMediaStateReceiver(e){let r=this.mediaStateReceivers,o=r.indexOf(e);o<0||r.splice(o,1)}enableHotkeys(){this.addEventListener("keydown",f(this,Ei,To))}disableHotkeys(){this.removeEventListener("keydown",f(this,Ei,To)),this.removeEventListener("keyup",s(this,De))}get hotkeys(){return s(this,pe)}set hotkeys(e){R(this,p.HOTKEYS,e)}keyboardShortcutHandler(e){var b,A,M,T,v,x,_,L,C;let r=e.target;if(((M=(A=(b=r.getAttribute(p.KEYS_USED))==null?void 0:b.split(" "))!=null?A:r==null?void 0:r.keysUsed)!=null?M:[]).map(O=>O==="Space"?" ":O).filter(Boolean).includes(e.key))return;let n,d,c;if(!(s(this,pe).contains(yr(e.key))||e.shiftKey&&(e.key==="/"||e.key==="?")&&s(this,pe).contains("noshift+/")))switch(e.key){case" ":case"k":n=s(this,w).getState().mediaPaused?E.MEDIA_PLAY_REQUEST:E.MEDIA_PAUSE_REQUEST,this.dispatchEvent(new l.CustomEvent(n,{composed:!0,bubbles:!0}));break;case"m":n=this.mediaStore.getState().mediaVolumeLevel==="off"?E.MEDIA_UNMUTE_REQUEST:E.MEDIA_MUTE_REQUEST,this.dispatchEvent(new l.CustomEvent(n,{composed:!0,bubbles:!0}));break;case"f":n=this.mediaStore.getState().mediaIsFullscreen?E.MEDIA_EXIT_FULLSCREEN_REQUEST:E.MEDIA_ENTER_FULLSCREEN_REQUEST,this.dispatchEvent(new l.CustomEvent(n,{composed:!0,bubbles:!0}));break;case"c":this.dispatchEvent(new l.CustomEvent(E.MEDIA_TOGGLE_SUBTITLES_REQUEST,{composed:!0,bubbles:!0}));break;case"ArrowLeft":case"j":{let O=this.hasAttribute(p.KEYBOARD_BACKWARD_SEEK_OFFSET)?+this.getAttribute(p.KEYBOARD_BACKWARD_SEEK_OFFSET):is;d=Math.max(((T=this.mediaStore.getState().mediaCurrentTime)!=null?T:0)-O,0),c=new l.CustomEvent(E.MEDIA_SEEK_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case"ArrowRight":case"l":{let O=this.hasAttribute(p.KEYBOARD_FORWARD_SEEK_OFFSET)?+this.getAttribute(p.KEYBOARD_FORWARD_SEEK_OFFSET):is;d=Math.max(((v=this.mediaStore.getState().mediaCurrentTime)!=null?v:0)+O,0),c=new l.CustomEvent(E.MEDIA_SEEK_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case"ArrowUp":{let O=this.hasAttribute(p.KEYBOARD_UP_VOLUME_STEP)?+this.getAttribute(p.KEYBOARD_UP_VOLUME_STEP):rs;d=Math.min(((x=this.mediaStore.getState().mediaVolume)!=null?x:1)+O,1),c=new l.CustomEvent(E.MEDIA_VOLUME_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case"ArrowDown":{let O=this.hasAttribute(p.KEYBOARD_DOWN_VOLUME_STEP)?+this.getAttribute(p.KEYBOARD_DOWN_VOLUME_STEP):rs;d=Math.max(((_=this.mediaStore.getState().mediaVolume)!=null?_:1)-O,0),c=new l.CustomEvent(E.MEDIA_VOLUME_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case"<":{let O=(L=this.mediaStore.getState().mediaPlaybackRate)!=null?L:1;d=Math.max(O-os,dd).toFixed(2),c=new l.CustomEvent(E.MEDIA_PLAYBACK_RATE_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case">":{let O=(C=this.mediaStore.getState().mediaPlaybackRate)!=null?C:1;d=Math.min(O+os,ld).toFixed(2),c=new l.CustomEvent(E.MEDIA_PLAYBACK_RATE_REQUEST,{composed:!0,bubbles:!0,detail:d}),this.dispatchEvent(c);break}case"/":case"?":{e.shiftKey&&f(this,Rr,as).call(this);break}case"p":{n=this.mediaStore.getState().mediaIsPip?E.MEDIA_EXIT_PIP_REQUEST:E.MEDIA_ENTER_PIP_REQUEST,c=new l.CustomEvent(n,{composed:!0,bubbles:!0}),this.dispatchEvent(c);break}default:break}}};pe=new WeakMap,ht=new WeakMap,w=new WeakMap,z=new WeakMap,pt=new WeakMap,re=new WeakMap,hi=new WeakMap,Et=new WeakMap,pi=new WeakSet,vo=function(){var e;this.mediaStore=es({media:this.media,fullscreenElement:this.fullscreenElement,options:{defaultSubtitles:this.hasAttribute(p.DEFAULT_SUBTITLES),defaultDuration:this.hasAttribute(p.DEFAULT_DURATION)?+this.getAttribute(p.DEFAULT_DURATION):void 0,defaultStreamType:(e=this.getAttribute(p.DEFAULT_STREAM_TYPE))!=null?e:void 0,liveEdgeOffset:this.hasAttribute(p.LIVE_EDGE_OFFSET)?+this.getAttribute(p.LIVE_EDGE_OFFSET):void 0,seekToLiveOffset:this.hasAttribute(p.SEEK_TO_LIVE_OFFSET)?+this.getAttribute(p.SEEK_TO_LIVE_OFFSET):this.hasAttribute(p.LIVE_EDGE_OFFSET)?+this.getAttribute(p.LIVE_EDGE_OFFSET):void 0,noAutoSeekToLive:this.hasAttribute(p.NO_AUTO_SEEK_TO_LIVE),noVolumePref:this.hasAttribute(p.NO_VOLUME_PREF),noMutedPref:this.hasAttribute(p.NO_MUTED_PREF),noSubtitlesLangPref:this.hasAttribute(p.NO_SUBTITLES_LANG_PREF)}})},De=new WeakMap,Ei=new WeakSet,To=function(e){var M;let{metaKey:r,altKey:o,ctrlKey:n,key:d,shiftKey:c}=e,m=c&&(d==="/"||d==="?");if(m&&((M=s(this,z))!=null&&M.open)){this.removeEventListener("keyup",s(this,De));return}if(r||o||n||!m&&!ts.includes(d)){this.removeEventListener("keyup",s(this,De));return}let b=e.target,A=b instanceof HTMLElement&&(b.tagName.toLowerCase()==="media-volume-range"||b.tagName.toLowerCase()==="media-time-range");[" ","ArrowLeft","ArrowRight","ArrowUp","ArrowDown"].includes(d)&&!s(this,pe).contains(yr(d))&&!A&&e.preventDefault(),this.addEventListener("keyup",s(this,De),{once:!0})},gt=new WeakSet,Lr=function(){s(this,z)&&(s(this,z).hotkeys=s(this,pe).value||void 0,(this.noHotkeys||s(this,pe).contains("noshift+/"))&&(s(this,z).open=!1))},Rr=new WeakSet,as=function(){s(this,z)||(h(this,z,K.createElement("media-keyboard-shortcuts-dialog")),f(this,gt,Lr).call(this),this.appendChild(s(this,z))),s(this,z).open=!0};var cd=Object.values(a),ud=Object.values(er),ds=i=>{var r,o,n,d;let{observedAttributes:t}=i.constructor;!t&&((r=i.nodeName)!=null&&r.includes("-"))&&(l.customElements.upgrade(i),{observedAttributes:t}=i.constructor);let e=(d=(n=(o=i==null?void 0:i.getAttribute)==null?void 0:o.call(i,k.MEDIA_CHROME_ATTRIBUTES))==null?void 0:n.split)==null?void 0:d.call(n,/\s+/);return Array.isArray(t||e)?(t||e).filter(c=>cd.includes(c)):[]},md=i=>{var t,e;return(t=i.nodeName)!=null&&t.includes("-")&&l.customElements.get((e=i.nodeName)==null?void 0:e.toLowerCase())&&!(i instanceof l.customElements.get(i.nodeName.toLowerCase()))&&l.customElements.upgrade(i),ud.some(r=>r in i)},Ao=i=>md(i)||!!ds(i).length,ns=i=>{var t;return(t=i==null?void 0:i.join)==null?void 0:t.call(i,":")},fo={[a.MEDIA_SUBTITLES_LIST]:ui,[a.MEDIA_SUBTITLES_SHOWING]:ui,[a.MEDIA_SEEKABLE]:ns,[a.MEDIA_BUFFERED]:i=>i==null?void 0:i.map(ns).join(" "),[a.MEDIA_PREVIEW_COORDS]:i=>i==null?void 0:i.join(" "),[a.MEDIA_RENDITION_LIST]:un,[a.MEDIA_AUDIO_TRACK_LIST]:mn},hd=async(i,t,e)=>{var o,n;if(i.isConnected||await ir(0),typeof e=="boolean"||e==null)return S(i,t,e);if(typeof e=="number")return H(i,t,e);if(typeof e=="string")return R(i,t,e);if(Array.isArray(e)&&!e.length)return i.removeAttribute(t);let r=(n=(o=fo[t])==null?void 0:o.call(fo,e))!=null?n:e;return i.setAttribute(t,r)},pd=i=>{var t;return!!((t=i.closest)!=null&&t.call(i,'*[slot="media"]'))},We=(i,t)=>{if(pd(i))return;let e=(o,n)=>{var b,A;Ao(o)&&n(o);let{children:d=[]}=o!=null?o:{},c=(A=(b=o==null?void 0:o.shadowRoot)==null?void 0:b.children)!=null?A:[];[...d,...c].forEach(M=>We(M,n))},r=i==null?void 0:i.nodeName.toLowerCase();if(r.includes("-")&&!Ao(i)){l.customElements.whenDefined(r).then(()=>{e(i,t)});return}e(i,t)},ss=(i,t,e)=>{i.forEach(r=>{if(t in r){r[t]=e;return}let o=ds(r),n=t.toLowerCase();o.includes(n)&&hd(r,n,e)})},Ed=(i,t,e)=>{We(i,t);let r=A=>{var T;let M=(T=A==null?void 0:A.composedPath()[0])!=null?T:A.target;t(M)},o=A=>{var T;let M=(T=A==null?void 0:A.composedPath()[0])!=null?T:A.target;e(M)};i.addEventListener(E.REGISTER_MEDIA_STATE_RECEIVER,r),i.addEventListener(E.UNREGISTER_MEDIA_STATE_RECEIVER,o);let n=A=>{A.forEach(M=>{let{addedNodes:T=[],removedNodes:v=[],type:x,target:_,attributeName:L}=M;x==="childList"?(Array.prototype.forEach.call(T,C=>We(C,t)),Array.prototype.forEach.call(v,C=>We(C,e))):x==="attributes"&&L===k.MEDIA_CHROME_ATTRIBUTES&&(Ao(_)?t(_):e(_))})},d=[],c=A=>{let M=A.target;M.name!=="media"&&(d.forEach(T=>We(T,e)),d=[...M.assignedElements({flatten:!0})],d.forEach(T=>We(T,t)))};i.addEventListener("slotchange",c);let m=new MutationObserver(n);return m.observe(i,{childList:!0,attributes:!0,subtree:!0}),()=>{We(i,e),i.removeEventListener("slotchange",c),m.disconnect(),i.removeEventListener(E.REGISTER_MEDIA_STATE_RECEIVER,r),i.removeEventListener(E.UNREGISTER_MEDIA_STATE_RECEIVER,o)}};l.customElements.get("media-controller")||l.customElements.define("media-controller",kr);var ls=kr;var bt={PLACEMENT:"placement",BOUNDS:"bounds"};function gd(i){return`
    <style>
      :host {
        --_tooltip-background-color: var(--media-tooltip-background-color, var(--media-secondary-color, rgba(20, 20, 30, .7)));
        --_tooltip-background: var(--media-tooltip-background, var(--_tooltip-background-color));
        --_tooltip-arrow-half-width: calc(var(--media-tooltip-arrow-width, 12px) / 2);
        --_tooltip-arrow-height: var(--media-tooltip-arrow-height, 5px);
        --_tooltip-arrow-background: var(--media-tooltip-arrow-color, var(--_tooltip-background-color));
        position: relative;
        pointer-events: none;
        display: var(--media-tooltip-display, inline-flex);
        justify-content: center;
        align-items: center;
        box-sizing: border-box;
        z-index: var(--media-tooltip-z-index, 1);
        background: var(--_tooltip-background);
        color: var(--media-text-color, var(--media-primary-color, rgb(238 238 238)));
        font: var(--media-font,
          var(--media-font-weight, 400)
          var(--media-font-size, 13px) /
          var(--media-text-content-height, var(--media-control-height, 18px))
          var(--media-font-family, helvetica neue, segoe ui, roboto, arial, sans-serif));
        padding: var(--media-tooltip-padding, .35em .7em);
        border: var(--media-tooltip-border, none);
        border-radius: var(--media-tooltip-border-radius, 5px);
        filter: var(--media-tooltip-filter, drop-shadow(0 0 4px rgba(0, 0, 0, .2)));
        white-space: var(--media-tooltip-white-space, nowrap);
      }

      :host([hidden]) {
        display: none;
      }

      img, svg {
        display: inline-block;
      }

      #arrow {
        position: absolute;
        width: 0px;
        height: 0px;
        border-style: solid;
        display: var(--media-tooltip-arrow-display, block);
      }

      :host(:not([placement])),
      :host([placement="top"]) {
        position: absolute;
        bottom: calc(100% + var(--media-tooltip-distance, 12px));
        left: 50%;
        transform: translate(calc(-50% - var(--media-tooltip-offset-x, 0px)), 0);
      }
      :host(:not([placement])) #arrow,
      :host([placement="top"]) #arrow {
        top: 100%;
        left: 50%;
        border-width: var(--_tooltip-arrow-height) var(--_tooltip-arrow-half-width) 0 var(--_tooltip-arrow-half-width);
        border-color: var(--_tooltip-arrow-background) transparent transparent transparent;
        transform: translate(calc(-50% + var(--media-tooltip-offset-x, 0px)), 0);
      }

      :host([placement="right"]) {
        position: absolute;
        left: calc(100% + var(--media-tooltip-distance, 12px));
        top: 50%;
        transform: translate(0, -50%);
      }
      :host([placement="right"]) #arrow {
        top: 50%;
        right: 100%;
        border-width: var(--_tooltip-arrow-half-width) var(--_tooltip-arrow-height) var(--_tooltip-arrow-half-width) 0;
        border-color: transparent var(--_tooltip-arrow-background) transparent transparent;
        transform: translate(0, -50%);
      }

      :host([placement="bottom"]) {
        position: absolute;
        top: calc(100% + var(--media-tooltip-distance, 12px));
        left: 50%;
        transform: translate(calc(-50% - var(--media-tooltip-offset-x, 0px)), 0);
      }
      :host([placement="bottom"]) #arrow {
        bottom: 100%;
        left: 50%;
        border-width: 0 var(--_tooltip-arrow-half-width) var(--_tooltip-arrow-height) var(--_tooltip-arrow-half-width);
        border-color: transparent transparent var(--_tooltip-arrow-background) transparent;
        transform: translate(calc(-50% + var(--media-tooltip-offset-x, 0px)), 0);
      }

      :host([placement="left"]) {
        position: absolute;
        right: calc(100% + var(--media-tooltip-distance, 12px));
        top: 50%;
        transform: translate(0, -50%);
      }
      :host([placement="left"]) #arrow {
        top: 50%;
        left: 100%;
        border-width: var(--_tooltip-arrow-half-width) 0 var(--_tooltip-arrow-half-width) var(--_tooltip-arrow-height);
        border-color: transparent transparent transparent var(--_tooltip-arrow-background);
        transform: translate(0, -50%);
      }
      
      :host([placement="none"]) #arrow {
        display: none;
      }
    </style>
    <slot></slot>
    <div id="arrow"></div>
  `}var ft=class extends l.HTMLElement{constructor(){super();this.updateXOffset=()=>{var C;if(!mr(this,{checkOpacity:!1,checkVisibilityCSS:!1}))return;let e=this.placement;if(e==="left"||e==="right"){this.style.removeProperty("--media-tooltip-offset-x");return}let r=getComputedStyle(this),o=(C=Me(this,"#"+this.bounds))!=null?C:Mn(this);if(!o)return;let{x:n,width:d}=o.getBoundingClientRect(),{x:c,width:m}=this.getBoundingClientRect(),b=c+m,A=n+d,M=r.getPropertyValue("--media-tooltip-offset-x"),T=M?parseFloat(M.replace("px","")):0,v=r.getPropertyValue("--media-tooltip-container-margin"),x=v?parseFloat(v.replace("px","")):0,_=c-n+T-x,L=b-A+T+x;if(_<0){this.style.setProperty("--media-tooltip-offset-x",`${_}px`);return}if(L>0){this.style.setProperty("--media-tooltip-offset-x",`${L}px`);return}this.style.removeProperty("--media-tooltip-offset-x")};if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}if(this.arrowEl=this.shadowRoot.querySelector("#arrow"),Object.prototype.hasOwnProperty.call(this,"placement")){let e=this.placement;delete this.placement,this.placement=e}}static get observedAttributes(){return[bt.PLACEMENT,bt.BOUNDS]}get placement(){return D(this,bt.PLACEMENT)}set placement(e){R(this,bt.PLACEMENT,e)}get bounds(){return D(this,bt.BOUNDS)}set bounds(e){R(this,bt.BOUNDS,e)}};ft.shadowRootOptions={mode:"open"},ft.getTemplateHTML=gd;l.customElements.get("media-tooltip")||l.customElements.define("media-tooltip",ft);var gi=ft;var xe={TOOLTIP_PLACEMENT:"tooltipplacement",DISABLED:"disabled",NO_TOOLTIP:"notooltip"};function bd(i,t={}){return`
    <style>
      :host {
        position: relative;
        font: var(--media-font,
          var(--media-font-weight, bold)
          var(--media-font-size, 14px) /
          var(--media-text-content-height, var(--media-control-height, 24px))
          var(--media-font-family, helvetica neue, segoe ui, roboto, arial, sans-serif));
        color: var(--media-text-color, var(--media-primary-color, rgb(238 238 238)));
        background: var(--media-control-background, var(--media-secondary-color, rgb(20 20 30 / .7)));
        padding: var(--media-button-padding, var(--media-control-padding, 10px));
        justify-content: var(--media-button-justify-content, center);
        display: inline-flex;
        align-items: center;
        vertical-align: middle;
        box-sizing: border-box;
        transition: background .15s linear;
        pointer-events: auto;
        cursor: var(--media-cursor, pointer);
        -webkit-tap-highlight-color: transparent;
      }

      
      :host(:focus-visible) {
        box-shadow: var(--media-focus-box-shadow, inset 0 0 0 2px rgb(27 127 204 / .9));
        outline: 0;
      }
      
      :host(:where(:focus)) {
        box-shadow: none;
        outline: 0;
      }

      :host(:hover) {
        background: var(--media-control-hover-background, rgba(50 50 70 / .7));
      }

      slot[name="icon"] {
        display: inline-flex;
        align-items: center;
      }

      svg, img, ::slotted(svg), ::slotted(img) {
        width: var(--media-button-icon-width);
        height: var(--media-button-icon-height, var(--media-control-height, 24px));
        transform: var(--media-button-icon-transform);
        transition: var(--media-button-icon-transition);
        fill: var(--media-icon-color, var(--media-primary-color, rgb(238 238 238)));
        vertical-align: middle;
        max-width: 100%;
        max-height: 100%;
        min-width: 100%;
      }

      media-tooltip {
        
        max-width: 0;
        overflow-x: clip;
        opacity: 0;
        transition: opacity .3s, max-width 0s 9s;
      }

      :host(:hover) media-tooltip,
      :host(:focus-visible) media-tooltip {
        max-width: 100vw;
        opacity: 1;
        transition: opacity .3s;
      }

      :host([notooltip]) slot[name="tooltip"] {
        display: none;
      }
    </style>

    ${this.getSlotTemplateHTML(i,t)}

    <slot name="tooltip">
      <media-tooltip part="tooltip" aria-hidden="true">
        <template shadowrootmode="${gi.shadowRootOptions.mode}">
          ${gi.getTemplateHTML({})}
        </template>
        <slot name="tooltip-content">
          ${this.getTooltipContentHTML(i)}
        </slot>
      </media-tooltip>
    </slot>
  `}function fd(i,t){return`
    <slot></slot>
  `}function vd(){return""}var oe,Ye,Dr,cs,Te,Qe,bi,xr,us,U=class extends l.HTMLElement{constructor(){super();u(this,Dr);u(this,xr);u(this,oe,void 0);this.preventClick=!1;this.tooltipEl=null;u(this,Ye,e=>{this.preventClick||this.handleClick(e),setTimeout(s(this,Te),0)});u(this,Te,()=>{var e,r;(r=(e=this.tooltipEl)==null?void 0:e.updateXOffset)==null||r.call(e)});u(this,Qe,e=>{let{key:r}=e;if(!this.keysUsed.includes(r)){this.removeEventListener("keyup",s(this,Qe));return}this.preventClick||this.handleClick(e)});u(this,bi,e=>{let{metaKey:r,altKey:o,key:n}=e;if(r||o||!this.keysUsed.includes(n)){this.removeEventListener("keyup",s(this,Qe));return}this.addEventListener("keyup",s(this,Qe),{once:!0})});if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes),r=this.constructor.getTemplateHTML(e);this.shadowRoot.setHTMLUnsafe?this.shadowRoot.setHTMLUnsafe(r):this.shadowRoot.innerHTML=r}this.tooltipEl=this.shadowRoot.querySelector("media-tooltip")}static get observedAttributes(){return["disabled",xe.TOOLTIP_PLACEMENT,k.MEDIA_CONTROLLER,a.MEDIA_LANG]}enable(){this.addEventListener("click",s(this,Ye)),this.addEventListener("keydown",s(this,bi)),this.tabIndex=0}disable(){this.removeEventListener("click",s(this,Ye)),this.removeEventListener("keydown",s(this,bi)),this.removeEventListener("keyup",s(this,Qe)),this.tabIndex=-1}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===k.MEDIA_CONTROLLER?(r&&((d=(n=s(this,oe))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,oe,null)),o&&this.isConnected&&(h(this,oe,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,oe))==null?void 0:m.associateElement)==null||b.call(m,this))):e==="disabled"&&o!==r?o==null?this.enable():this.disable():e===xe.TOOLTIP_PLACEMENT&&this.tooltipEl&&o!==r?this.tooltipEl.placement=o:e===a.MEDIA_LANG&&(this.shadowRoot.querySelector('slot[name="tooltip-content"]').innerHTML=this.constructor.getTooltipContentHTML()),f(this,Dr,cs).call(this)&&s(this,Te).call(this)}connectedCallback(){var o,n,d;let{style:e}=F(this.shadowRoot,":host");e.setProperty("display",`var(--media-control-display, var(--${this.localName}-display, inline-flex))`),this.hasAttribute("disabled")?this.disable():this.enable(),this.setAttribute("role","button");let r=this.getAttribute(k.MEDIA_CONTROLLER);r&&(h(this,oe,(o=this.getRootNode())==null?void 0:o.getElementById(r)),(d=(n=s(this,oe))==null?void 0:n.associateElement)==null||d.call(n,this)),l.customElements.whenDefined("media-tooltip").then(()=>f(this,xr,us).call(this))}disconnectedCallback(){var e,r;this.disable(),(r=(e=s(this,oe))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,oe,null),this.removeEventListener("mouseenter",s(this,Te)),this.removeEventListener("focus",s(this,Te)),this.removeEventListener("click",s(this,Ye))}get keysUsed(){return["Enter"," "]}get tooltipPlacement(){return D(this,xe.TOOLTIP_PLACEMENT)}set tooltipPlacement(e){R(this,xe.TOOLTIP_PLACEMENT,e)}get mediaController(){return D(this,k.MEDIA_CONTROLLER)}set mediaController(e){R(this,k.MEDIA_CONTROLLER,e)}get disabled(){return y(this,xe.DISABLED)}set disabled(e){S(this,xe.DISABLED,e)}get noTooltip(){return y(this,xe.NO_TOOLTIP)}set noTooltip(e){S(this,xe.NO_TOOLTIP,e)}handleClick(e){}};oe=new WeakMap,Ye=new WeakMap,Dr=new WeakSet,cs=function(){try{return this.matches(":hover, :focus-visible")}catch{return this.matches(":hover, :focus")}},Te=new WeakMap,Qe=new WeakMap,bi=new WeakMap,xr=new WeakSet,us=function(){this.addEventListener("mouseenter",s(this,Te)),this.addEventListener("focus",s(this,Te)),this.addEventListener("click",s(this,Ye));let e=this.tooltipPlacement;e&&this.tooltipEl&&(this.tooltipEl.placement=e)},U.shadowRootOptions={mode:"open"},U.getTemplateHTML=bd,U.getSlotTemplateHTML=fd,U.getTooltipContentHTML=vd;l.customElements.get("media-chrome-button")||l.customElements.define("media-chrome-button",U);var ms=U;var hs=`<svg aria-hidden="true" viewBox="0 0 26 24">
  <path d="M22.13 3H3.87a.87.87 0 0 0-.87.87v13.26a.87.87 0 0 0 .87.87h3.4L9 16H5V5h16v11h-4l1.72 2h3.4a.87.87 0 0 0 .87-.87V3.87a.87.87 0 0 0-.86-.87Zm-8.75 11.44a.5.5 0 0 0-.76 0l-4.91 5.73a.5.5 0 0 0 .38.83h9.82a.501.501 0 0 0 .38-.83l-4.91-5.73Z"/>
</svg>
`;function Td(i){return`
    <style>
      :host([${a.MEDIA_IS_AIRPLAYING}]) slot[name=icon] slot:not([name=exit]) {
        display: none !important;
      }

      
      :host(:not([${a.MEDIA_IS_AIRPLAYING}])) slot[name=icon] slot:not([name=enter]) {
        display: none !important;
      }

      :host([${a.MEDIA_IS_AIRPLAYING}]) slot[name=tooltip-enter],
      :host(:not([${a.MEDIA_IS_AIRPLAYING}])) slot[name=tooltip-exit] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="enter">${hs}</slot>
      <slot name="exit">${hs}</slot>
    </slot>
  `}function Ad(){return`
    <slot name="tooltip-enter">${g("start airplay")}</slot>
    <slot name="tooltip-exit">${g("stop airplay")}</slot>
  `}var ps=i=>{let t=i.mediaIsAirplaying?g("stop airplay"):g("start airplay");i.setAttribute("aria-label",t)},vt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_IS_AIRPLAYING,a.MEDIA_AIRPLAY_UNAVAILABLE]}connectedCallback(){super.connectedCallback(),ps(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),t===a.MEDIA_IS_AIRPLAYING&&ps(this)}get mediaIsAirplaying(){return y(this,a.MEDIA_IS_AIRPLAYING)}set mediaIsAirplaying(t){S(this,a.MEDIA_IS_AIRPLAYING,t)}get mediaAirplayUnavailable(){return D(this,a.MEDIA_AIRPLAY_UNAVAILABLE)}set mediaAirplayUnavailable(t){R(this,a.MEDIA_AIRPLAY_UNAVAILABLE,t)}handleClick(){let t=new l.CustomEvent(E.MEDIA_AIRPLAY_REQUEST,{composed:!0,bubbles:!0});this.dispatchEvent(t)}};vt.getSlotTemplateHTML=Td,vt.getTooltipContentHTML=Ad;l.customElements.get("media-airplay-button")||l.customElements.define("media-airplay-button",vt);var Es=vt;var yd=`<svg aria-hidden="true" viewBox="0 0 26 24">
  <path d="M22.83 5.68a2.58 2.58 0 0 0-2.3-2.5c-3.62-.24-11.44-.24-15.06 0a2.58 2.58 0 0 0-2.3 2.5c-.23 4.21-.23 8.43 0 12.64a2.58 2.58 0 0 0 2.3 2.5c3.62.24 11.44.24 15.06 0a2.58 2.58 0 0 0 2.3-2.5c.23-4.21.23-8.43 0-12.64Zm-11.39 9.45a3.07 3.07 0 0 1-1.91.57 3.06 3.06 0 0 1-2.34-1 3.75 3.75 0 0 1-.92-2.67 3.92 3.92 0 0 1 .92-2.77 3.18 3.18 0 0 1 2.43-1 2.94 2.94 0 0 1 2.13.78c.364.359.62.813.74 1.31l-1.43.35a1.49 1.49 0 0 0-1.51-1.17 1.61 1.61 0 0 0-1.29.58 2.79 2.79 0 0 0-.5 1.89 3 3 0 0 0 .49 1.93 1.61 1.61 0 0 0 1.27.58 1.48 1.48 0 0 0 1-.37 2.1 2.1 0 0 0 .59-1.14l1.4.44a3.23 3.23 0 0 1-1.07 1.69Zm7.22 0a3.07 3.07 0 0 1-1.91.57 3.06 3.06 0 0 1-2.34-1 3.75 3.75 0 0 1-.92-2.67 3.88 3.88 0 0 1 .93-2.77 3.14 3.14 0 0 1 2.42-1 3 3 0 0 1 2.16.82 2.8 2.8 0 0 1 .73 1.31l-1.43.35a1.49 1.49 0 0 0-1.51-1.21 1.61 1.61 0 0 0-1.29.58A2.79 2.79 0 0 0 15 12a3 3 0 0 0 .49 1.93 1.61 1.61 0 0 0 1.27.58 1.44 1.44 0 0 0 1-.37 2.1 2.1 0 0 0 .6-1.15l1.4.44a3.17 3.17 0 0 1-1.1 1.7Z"/>
</svg>`,Sd=`<svg aria-hidden="true" viewBox="0 0 26 24">
  <path d="M17.73 14.09a1.4 1.4 0 0 1-1 .37 1.579 1.579 0 0 1-1.27-.58A3 3 0 0 1 15 12a2.8 2.8 0 0 1 .5-1.85 1.63 1.63 0 0 1 1.29-.57 1.47 1.47 0 0 1 1.51 1.2l1.43-.34A2.89 2.89 0 0 0 19 9.07a3 3 0 0 0-2.14-.78 3.14 3.14 0 0 0-2.42 1 3.91 3.91 0 0 0-.93 2.78 3.74 3.74 0 0 0 .92 2.66 3.07 3.07 0 0 0 2.34 1 3.07 3.07 0 0 0 1.91-.57 3.17 3.17 0 0 0 1.07-1.74l-1.4-.45c-.083.43-.3.822-.62 1.12Zm-7.22 0a1.43 1.43 0 0 1-1 .37 1.58 1.58 0 0 1-1.27-.58A3 3 0 0 1 7.76 12a2.8 2.8 0 0 1 .5-1.85 1.63 1.63 0 0 1 1.29-.57 1.47 1.47 0 0 1 1.51 1.2l1.43-.34a2.81 2.81 0 0 0-.74-1.32 2.94 2.94 0 0 0-2.13-.78 3.18 3.18 0 0 0-2.43 1 4 4 0 0 0-.92 2.78 3.74 3.74 0 0 0 .92 2.66 3.07 3.07 0 0 0 2.34 1 3.07 3.07 0 0 0 1.91-.57 3.23 3.23 0 0 0 1.07-1.74l-1.4-.45a2.06 2.06 0 0 1-.6 1.07Zm12.32-8.41a2.59 2.59 0 0 0-2.3-2.51C18.72 3.05 15.86 3 13 3c-2.86 0-5.72.05-7.53.17a2.59 2.59 0 0 0-2.3 2.51c-.23 4.207-.23 8.423 0 12.63a2.57 2.57 0 0 0 2.3 2.5c1.81.13 4.67.19 7.53.19 2.86 0 5.72-.06 7.53-.19a2.57 2.57 0 0 0 2.3-2.5c.23-4.207.23-8.423 0-12.63Zm-1.49 12.53a1.11 1.11 0 0 1-.91 1.11c-1.67.11-4.45.18-7.43.18-2.98 0-5.76-.07-7.43-.18a1.11 1.11 0 0 1-.91-1.11c-.21-4.14-.21-8.29 0-12.43a1.11 1.11 0 0 1 .91-1.11C7.24 4.56 10 4.49 13 4.49s5.76.07 7.43.18a1.11 1.11 0 0 1 .91 1.11c.21 4.14.21 8.29 0 12.43Z"/>
</svg>`;function Id(i){return`
    <style>
      :host([aria-checked="true"]) slot[name=off] {
        display: none !important;
      }

      
      :host(:not([aria-checked="true"])) slot[name=on] {
        display: none !important;
      }

      :host([aria-checked="true"]) slot[name=tooltip-enable],
      :host(:not([aria-checked="true"])) slot[name=tooltip-disable] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="on">${yd}</slot>
      <slot name="off">${Sd}</slot>
    </slot>
  `}function Md(){return`
    <slot name="tooltip-enable">${g("Enable captions")}</slot>
    <slot name="tooltip-disable">${g("Disable captions")}</slot>
  `}var gs=i=>{i.setAttribute("aria-checked",Bn(i).toString())},Tt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_SUBTITLES_LIST,a.MEDIA_SUBTITLES_SHOWING]}connectedCallback(){super.connectedCallback(),this.setAttribute("role","switch"),this.setAttribute("aria-label",g("closed captions")),gs(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),t===a.MEDIA_SUBTITLES_SHOWING&&gs(this)}get mediaSubtitlesList(){return bs(this,a.MEDIA_SUBTITLES_LIST)}set mediaSubtitlesList(t){fs(this,a.MEDIA_SUBTITLES_LIST,t)}get mediaSubtitlesShowing(){return bs(this,a.MEDIA_SUBTITLES_SHOWING)}set mediaSubtitlesShowing(t){fs(this,a.MEDIA_SUBTITLES_SHOWING,t)}handleClick(){this.dispatchEvent(new l.CustomEvent(E.MEDIA_TOGGLE_SUBTITLES_REQUEST,{composed:!0,bubbles:!0}))}};Tt.getSlotTemplateHTML=Id,Tt.getTooltipContentHTML=Md;var bs=(i,t)=>{let e=i.getAttribute(t);return e?mo(e):[]},fs=(i,t,e)=>{if(!(e!=null&&e.length)){i.removeAttribute(t);return}let r=ui(e);i.getAttribute(t)!==r&&i.setAttribute(t,r)};l.customElements.get("media-captions-button")||l.customElements.define("media-captions-button",Tt);var vs=Tt;var _d='<svg aria-hidden="true" viewBox="0 0 24 24"><g><path class="cast_caf_icon_arch0" d="M1,18 L1,21 L4,21 C4,19.3 2.66,18 1,18 L1,18 Z"/><path class="cast_caf_icon_arch1" d="M1,14 L1,16 C3.76,16 6,18.2 6,21 L8,21 C8,17.13 4.87,14 1,14 L1,14 Z"/><path class="cast_caf_icon_arch2" d="M1,10 L1,12 C5.97,12 10,16.0 10,21 L12,21 C12,14.92 7.07,10 1,10 L1,10 Z"/><path class="cast_caf_icon_box" d="M21,3 L3,3 C1.9,3 1,3.9 1,5 L1,8 L3,8 L3,5 L21,5 L21,19 L14,19 L14,21 L21,21 C22.1,21 23,20.1 23,19 L23,5 C23,3.9 22.1,3 21,3 L21,3 Z"/></g></svg>',Ld='<svg aria-hidden="true" viewBox="0 0 24 24"><g><path class="cast_caf_icon_arch0" d="M1,18 L1,21 L4,21 C4,19.3 2.66,18 1,18 L1,18 Z"/><path class="cast_caf_icon_arch1" d="M1,14 L1,16 C3.76,16 6,18.2 6,21 L8,21 C8,17.13 4.87,14 1,14 L1,14 Z"/><path class="cast_caf_icon_arch2" d="M1,10 L1,12 C5.97,12 10,16.0 10,21 L12,21 C12,14.92 7.07,10 1,10 L1,10 Z"/><path class="cast_caf_icon_box" d="M21,3 L3,3 C1.9,3 1,3.9 1,5 L1,8 L3,8 L3,5 L21,5 L21,19 L14,19 L14,21 L21,21 C22.1,21 23,20.1 23,19 L23,5 C23,3.9 22.1,3 21,3 L21,3 Z"/><path class="cast_caf_icon_boxfill" d="M5,7 L5,8.63 C8,8.6 13.37,14 13.37,17 L19,17 L19,7 Z"/></g></svg>';function kd(i){return`
    <style>
      :host([${a.MEDIA_IS_CASTING}]) slot[name=icon] slot:not([name=exit]) {
        display: none !important;
      }

      
      :host(:not([${a.MEDIA_IS_CASTING}])) slot[name=icon] slot:not([name=enter]) {
        display: none !important;
      }

      :host([${a.MEDIA_IS_CASTING}]) slot[name=tooltip-enter],
      :host(:not([${a.MEDIA_IS_CASTING}])) slot[name=tooltip-exit] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="enter">${_d}</slot>
      <slot name="exit">${Ld}</slot>
    </slot>
  `}function Rd(){return`
    <slot name="tooltip-enter">${g("Start casting")}</slot>
    <slot name="tooltip-exit">${g("Stop casting")}</slot>
  `}var Ts=i=>{let t=i.mediaIsCasting?g("stop casting"):g("start casting");i.setAttribute("aria-label",t)},At=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_IS_CASTING,a.MEDIA_CAST_UNAVAILABLE]}connectedCallback(){super.connectedCallback(),Ts(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),t===a.MEDIA_IS_CASTING&&Ts(this)}get mediaIsCasting(){return y(this,a.MEDIA_IS_CASTING)}set mediaIsCasting(t){S(this,a.MEDIA_IS_CASTING,t)}get mediaCastUnavailable(){return D(this,a.MEDIA_CAST_UNAVAILABLE)}set mediaCastUnavailable(t){R(this,a.MEDIA_CAST_UNAVAILABLE,t)}handleClick(){let t=this.mediaIsCasting?E.MEDIA_EXIT_CAST_REQUEST:E.MEDIA_ENTER_CAST_REQUEST;this.dispatchEvent(new l.CustomEvent(t,{composed:!0,bubbles:!0}))}};At.getSlotTemplateHTML=kd,At.getTooltipContentHTML=Rd;l.customElements.get("media-cast-button")||l.customElements.define("media-cast-button",At);var As=At;function Dd(i){return`
    <style>
      :host {
        --_focus-box-shadow: var(--media-focus-box-shadow, inset 0 0 0 2px rgb(27 127 204 / .9));
        --_media-range-padding: var(--media-range-padding, var(--media-control-padding, 10px));

        box-shadow: var(--_focus-visible-box-shadow, none);
        background: var(--media-control-background, var(--media-secondary-color, rgb(20 20 30 / .7)));
        height: calc(var(--media-control-height, 24px) + 2 * var(--_media-range-padding));
        display: inline-flex;
        align-items: center;
        
        vertical-align: middle;
        box-sizing: border-box;
        position: relative;
        width: 100px;
        transition: background .15s linear;
        cursor: var(--media-cursor, pointer);
        pointer-events: auto;
        touch-action: none; 
      }

      
      input[type=range]:focus {
        outline: 0;
      }
      input[type=range]:focus::-webkit-slider-runnable-track {
        outline: 0;
      }

      :host(:hover) {
        background: var(--media-control-hover-background, rgb(50 50 70 / .7));
      }

      #leftgap {
        padding-left: var(--media-range-padding-left, var(--_media-range-padding));
      }

      #rightgap {
        padding-right: var(--media-range-padding-right, var(--_media-range-padding));
      }

      #startpoint,
      #endpoint {
        position: absolute;
      }

      #endpoint {
        right: 0;
      }

      #container {
        
        width: var(--media-range-track-width, 100%);
        transform: translate(var(--media-range-track-translate-x, 0px), var(--media-range-track-translate-y, 0px));
        position: relative;
        height: 100%;
        display: flex;
        align-items: center;
        min-width: 40px;
      }

      #range {
        
        display: var(--media-time-range-hover-display, block);
        bottom: var(--media-time-range-hover-bottom, 0);
        height: var(--media-time-range-hover-height, max(100% , 25px));
        width: 100%;
        position: absolute;
        cursor: var(--media-cursor, pointer);

        -webkit-appearance: none; 
        -webkit-tap-highlight-color: transparent;
        background: transparent; 
        margin: 0;
        z-index: 1;
      }

      @media (hover: hover) {
        #range {
          bottom: var(--media-time-range-hover-bottom, 0);
          height: var(--media-time-range-hover-height, max(100%, 20px));
        }
      }

      
      
      #range::-webkit-slider-thumb {
        -webkit-appearance: none;
        background: transparent;
        width: .1px;
        height: .1px;
      }

      
      #range::-moz-range-thumb {
        background: transparent;
        border: transparent;
        width: .1px;
        height: .1px;
      }

      #appearance {
        height: var(--media-range-track-height, 4px);
        display: flex;
        flex-direction: column;
        justify-content: center;
        width: 100%;
        position: absolute;
        
        will-change: transform;
      }

      #track {
        background: var(--media-range-track-background, rgb(255 255 255 / .2));
        border-radius: var(--media-range-track-border-radius, 1px);
        border: var(--media-range-track-border, none);
        outline: var(--media-range-track-outline);
        outline-offset: var(--media-range-track-outline-offset);
        backdrop-filter: var(--media-range-track-backdrop-filter);
        -webkit-backdrop-filter: var(--media-range-track-backdrop-filter);
        box-shadow: var(--media-range-track-box-shadow, none);
        position: absolute;
        width: 100%;
        height: 100%;
        overflow: hidden;
      }

      #progress,
      #pointer {
        position: absolute;
        height: 100%;
        will-change: width;
      }

      #progress {
        background: var(--media-range-bar-color, var(--media-primary-color, rgb(238 238 238)));
        transition: var(--media-range-track-transition);
      }

      #pointer {
        background: var(--media-range-track-pointer-background);
        border-right: var(--media-range-track-pointer-border-right);
        transition: visibility .25s, opacity .25s;
        visibility: hidden;
        opacity: 0;
      }

      @media (hover: hover) {
        :host(:hover) #pointer {
          transition: visibility .5s, opacity .5s;
          visibility: visible;
          opacity: 1;
        }
      }

      #thumb,
      ::slotted([slot=thumb]) {
        width: var(--media-range-thumb-width, 10px);
        height: var(--media-range-thumb-height, 10px);
        transition: var(--media-range-thumb-transition);
        transform: var(--media-range-thumb-transform, none);
        opacity: var(--media-range-thumb-opacity, 1);
        translate: -50%;
        position: absolute;
        left: 0;
        cursor: var(--media-cursor, pointer);
      }

      #thumb {
        border-radius: var(--media-range-thumb-border-radius, 10px);
        background: var(--media-range-thumb-background, var(--media-primary-color, rgb(238 238 238)));
        box-shadow: var(--media-range-thumb-box-shadow, 1px 1px 1px transparent);
        border: var(--media-range-thumb-border, none);
      }

      :host([disabled]) #thumb {
        background-color: #777;
      }

      .segments #appearance {
        height: var(--media-range-segment-hover-height, 7px);
      }

      #track {
        clip-path: url(#segments-clipping);
      }

      #segments {
        --segments-gap: var(--media-range-segments-gap, 2px);
        position: absolute;
        width: 100%;
        height: 100%;
      }

      #segments-clipping {
        transform: translateX(calc(var(--segments-gap) / 2));
      }

      #segments-clipping:empty {
        display: none;
      }

      #segments-clipping rect {
        height: var(--media-range-track-height, 4px);
        y: calc((var(--media-range-segment-hover-height, 7px) - var(--media-range-track-height, 4px)) / 2);
        transition: var(--media-range-segment-transition, transform .1s ease-in-out);
        transform: var(--media-range-segment-transform, scaleY(1));
        transform-origin: center;
      }

      /* Visible label for accessibility - positioned off-screen but technically visible (Firefox requires visible labels) */
      #range-label {
        position: absolute;
        left: -10000px;
        background: var(--media-control-background, var(--media-secondary-color, rgb(20 20 30 / .7)));
        pointer-events: none;
      }
    </style>
    <div id="leftgap"></div>
    <div id="container">
      <div id="startpoint"></div>
      <div id="endpoint"></div>
      <div id="appearance">
        <div id="track" part="track">
          <div id="pointer"></div>
          <div id="progress" part="progress"></div>
        </div>
        <slot name="thumb">
          <div id="thumb" part="thumb"></div>
        </slot>
        <svg id="segments" aria-hidden="true"><clipPath id="segments-clipping"></clipPath></svg>
      </div>
        <input id="range" type="range" min="0" max="1" step="any" value="0">
        <label for="range" id="range-label"></label>

      ${this.getContainerTemplateHTML(i)}
    </div>
    <div id="rightgap"></div>
  `}function xd(i){return""}var ne,fi,vi,Ti,J,Ai,yi,Si,Ii,wr,ys,Mi,yo,_i,So,Li,Io,Cr,Ss,Or,Is,Pr,Ms,Ur,_s,se=class extends l.HTMLElement{constructor(){super();u(this,wr);u(this,Mi);u(this,_i);u(this,Li);u(this,Cr);u(this,Or);u(this,Pr);u(this,Ur);u(this,ne,void 0);u(this,fi,void 0);u(this,vi,void 0);u(this,Ti,void 0);u(this,J,{});u(this,Ai,[]);u(this,yi,()=>{if(this.range.matches(":focus-visible")){let{style:e}=F(this.shadowRoot,":host");e.setProperty("--_focus-visible-box-shadow","var(--_focus-box-shadow)")}});u(this,Si,()=>{let{style:e}=F(this.shadowRoot,":host");e.removeProperty("--_focus-visible-box-shadow")});u(this,Ii,()=>{let e=this.shadowRoot.querySelector("#segments-clipping");e&&e.parentNode.append(e)});if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes),r=this.constructor.getTemplateHTML(e);this.shadowRoot.setHTMLUnsafe?this.shadowRoot.setHTMLUnsafe(r):this.shadowRoot.innerHTML=r}this.container=this.shadowRoot.querySelector("#container"),h(this,vi,this.shadowRoot.querySelector("#startpoint")),h(this,Ti,this.shadowRoot.querySelector("#endpoint")),this.range=this.shadowRoot.querySelector("#range"),this.appearance=this.shadowRoot.querySelector("#appearance")}static get observedAttributes(){return["disabled","aria-disabled",k.MEDIA_CONTROLLER]}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===k.MEDIA_CONTROLLER?(r&&((d=(n=s(this,ne))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,ne,null)),o&&this.isConnected&&(h(this,ne,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,ne))==null?void 0:m.associateElement)==null||b.call(m,this))):(e==="disabled"||e==="aria-disabled"&&r!==o)&&(o==null?(this.range.removeAttribute(e),f(this,Mi,yo).call(this)):(this.range.setAttribute(e,o),f(this,_i,So).call(this)))}connectedCallback(){var o,n,d;let{style:e}=F(this.shadowRoot,":host");e.setProperty("display",`var(--media-control-display, var(--${this.localName}-display, inline-flex))`),s(this,J).pointer=F(this.shadowRoot,"#pointer"),s(this,J).progress=F(this.shadowRoot,"#progress"),s(this,J).thumb=F(this.shadowRoot,'#thumb, ::slotted([slot="thumb"])'),s(this,J).activeSegment=F(this.shadowRoot,"#segments-clipping rect:nth-child(0)");let r=this.getAttribute(k.MEDIA_CONTROLLER);r&&(h(this,ne,(o=this.getRootNode())==null?void 0:o.getElementById(r)),(d=(n=s(this,ne))==null?void 0:n.associateElement)==null||d.call(n,this)),this.updateBar(),this.shadowRoot.addEventListener("focusin",s(this,yi)),this.shadowRoot.addEventListener("focusout",s(this,Si)),f(this,Mi,yo).call(this),dr(this.container,s(this,Ii))}disconnectedCallback(){var e,r;f(this,_i,So).call(this),(r=(e=s(this,ne))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,ne,null),this.shadowRoot.removeEventListener("focusin",s(this,yi)),this.shadowRoot.removeEventListener("focusout",s(this,Si)),lr(this.container,s(this,Ii))}updatePointerBar(e){var r;(r=s(this,J).pointer)==null||r.style.setProperty("width",`${this.getPointerRatio(e)*100}%`)}updateBar(){var r,o;let e=this.range.valueAsNumber*100;(r=s(this,J).progress)==null||r.style.setProperty("width",`${e}%`),(o=s(this,J).thumb)==null||o.style.setProperty("left",`${e}%`)}updateSegments(e){let r=this.shadowRoot.querySelector("#segments-clipping");if(r.textContent="",this.container.classList.toggle("segments",!!(e!=null&&e.length)),!(e!=null&&e.length))return;let o=[...new Set([+this.range.min,...e.flatMap(d=>[d.start,d.end]),+this.range.max])];h(this,Ai,[...o]);let n=o.pop();for(let[d,c]of o.entries()){let[m,b]=[d===0,d===o.length-1],A=m?"calc(var(--segments-gap) / -1)":`${c*100}%`,T=`calc(${((b?n:o[d+1])-c)*100}%${m||b?"":" - var(--segments-gap)"})`,v=K.createElementNS("http://www.w3.org/2000/svg","rect"),x=lo(this.shadowRoot,`#segments-clipping rect:nth-child(${d+1})`);x.style.setProperty("x",A),x.style.setProperty("width",T),r.append(v)}}getPointerRatio(e){return _n(e.clientX,e.clientY,s(this,vi).getBoundingClientRect(),s(this,Ti).getBoundingClientRect())}get dragging(){return this.hasAttribute("dragging")}handleEvent(e){switch(e.type){case"pointermove":f(this,Ur,_s).call(this,e);break;case"input":this.updateBar();break;case"pointerenter":f(this,Cr,Ss).call(this,e);break;case"pointerdown":f(this,Li,Io).call(this,e);break;case"pointerup":f(this,Or,Is).call(this);break;case"pointerleave":f(this,Pr,Ms).call(this);break}}get keysUsed(){return["ArrowUp","ArrowRight","ArrowDown","ArrowLeft"]}};ne=new WeakMap,fi=new WeakMap,vi=new WeakMap,Ti=new WeakMap,J=new WeakMap,Ai=new WeakMap,yi=new WeakMap,Si=new WeakMap,Ii=new WeakMap,wr=new WeakSet,ys=function(e){let r=s(this,J).activeSegment;if(!r)return;let o=this.getPointerRatio(e),d=`#segments-clipping rect:nth-child(${s(this,Ai).findIndex((c,m,b)=>{let A=b[m+1];return A!=null&&o>=c&&o<=A})+1})`;(r.selectorText!=d||!r.style.transform)&&(r.selectorText=d,r.style.setProperty("transform","var(--media-range-segment-hover-transform, scaleY(2))"))},Mi=new WeakSet,yo=function(){this.hasAttribute("disabled")||!this.isConnected||(this.addEventListener("input",this),this.addEventListener("pointerdown",this),this.addEventListener("pointerenter",this))},_i=new WeakSet,So=function(){var e,r;this.removeEventListener("input",this),this.removeEventListener("pointerdown",this),this.removeEventListener("pointerenter",this),this.removeEventListener("pointerleave",this),(e=l.window)==null||e.removeEventListener("pointerup",this),(r=l.window)==null||r.removeEventListener("pointermove",this)},Li=new WeakSet,Io=function(e){var r;h(this,fi,e.composedPath().includes(this.range)),(r=l.window)==null||r.addEventListener("pointerup",this,{once:!0})},Cr=new WeakSet,Ss=function(e){var r;e.pointerType!=="mouse"&&f(this,Li,Io).call(this,e),this.addEventListener("pointerleave",this,{once:!0}),(r=l.window)==null||r.addEventListener("pointermove",this)},Or=new WeakSet,Is=function(){var e;(e=l.window)==null||e.removeEventListener("pointerup",this),this.toggleAttribute("dragging",!1),this.range.disabled=this.hasAttribute("disabled")},Pr=new WeakSet,Ms=function(){var e,r;this.removeEventListener("pointerleave",this),(e=l.window)==null||e.removeEventListener("pointermove",this),this.toggleAttribute("dragging",!1),this.range.disabled=this.hasAttribute("disabled"),(r=s(this,J).activeSegment)==null||r.style.removeProperty("transform")},Ur=new WeakSet,_s=function(e){e.pointerType==="pen"&&e.buttons===0||(this.toggleAttribute("dragging",e.buttons===1||e.pointerType!=="mouse"),this.updatePointerBar(e),f(this,wr,ys).call(this,e),this.dragging&&(e.pointerType!=="mouse"||!s(this,fi))&&(this.range.disabled=!0,this.range.valueAsNumber=this.getPointerRatio(e),this.range.dispatchEvent(new Event("input",{bubbles:!0,composed:!0}))))},se.shadowRootOptions={mode:"open"},se.getTemplateHTML=Dd,se.getContainerTemplateHTML=xd;l.customElements.get("media-chrome-range")||l.customElements.define("media-chrome-range",se);var Ls=se;function wd(i){return`
    <style>
      :host {
        
        box-sizing: border-box;
        display: var(--media-control-display, var(--media-control-bar-display, inline-flex));
        color: var(--media-text-color, var(--media-primary-color, rgb(238 238 238)));
        --media-loading-indicator-icon-height: 44px;
      }

      ::slotted(media-time-range),
      ::slotted(media-volume-range) {
        min-height: 100%;
      }

      ::slotted(media-time-range),
      ::slotted(media-clip-selector) {
        flex-grow: 1;
      }

      ::slotted([role="menu"]) {
        position: absolute;
      }
    </style>

    <slot></slot>
  `}var ae,yt=class extends l.HTMLElement{constructor(){super();u(this,ae,void 0);if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}}static get observedAttributes(){return[k.MEDIA_CONTROLLER]}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===k.MEDIA_CONTROLLER&&(r&&((d=(n=s(this,ae))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,ae,null)),o&&this.isConnected&&(h(this,ae,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,ae))==null?void 0:m.associateElement)==null||b.call(m,this)))}connectedCallback(){var r,o,n;let e=this.getAttribute(k.MEDIA_CONTROLLER);e&&(h(this,ae,(r=this.getRootNode())==null?void 0:r.getElementById(e)),(n=(o=s(this,ae))==null?void 0:o.associateElement)==null||n.call(o,this))}disconnectedCallback(){var e,r;(r=(e=s(this,ae))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,ae,null)}};ae=new WeakMap,yt.shadowRootOptions={mode:"open"},yt.getTemplateHTML=wd;l.customElements.get("media-control-bar")||l.customElements.define("media-control-bar",yt);var ks=yt;function Cd(i,t={}){return`
    <style>
      :host {
        font: var(--media-font,
          var(--media-font-weight, normal)
          var(--media-font-size, 14px) /
          var(--media-text-content-height, var(--media-control-height, 24px))
          var(--media-font-family, helvetica neue, segoe ui, roboto, arial, sans-serif));
        color: var(--media-text-color, var(--media-primary-color, rgb(238 238 238)));
        background: var(--media-text-background, var(--media-control-background, var(--media-secondary-color, rgb(20 20 30 / .7))));
        padding: var(--media-control-padding, 10px);
        display: inline-flex;
        justify-content: center;
        align-items: center;
        vertical-align: middle;
        box-sizing: border-box;
        text-align: center;
        pointer-events: auto;
      }

      
      :host(:focus-visible) {
        box-shadow: var(--media-focus-box-shadow, inset 0 0 0 2px rgb(27 127 204 / .9));
        outline: 0;
      }

      
      :host(:where(:focus)) {
        box-shadow: none;
        outline: 0;
      }
    </style>

    ${this.getSlotTemplateHTML(i,t)}
  `}function Od(i,t){return`
    <slot></slot>
  `}var de,Q=class extends l.HTMLElement{constructor(){super();u(this,de,void 0);if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}}static get observedAttributes(){return[k.MEDIA_CONTROLLER]}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===k.MEDIA_CONTROLLER&&(r&&((d=(n=s(this,de))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,de,null)),o&&this.isConnected&&(h(this,de,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,de))==null?void 0:m.associateElement)==null||b.call(m,this)))}connectedCallback(){var o,n,d;let{style:e}=F(this.shadowRoot,":host");e.setProperty("display",`var(--media-control-display, var(--${this.localName}-display, inline-flex))`);let r=this.getAttribute(k.MEDIA_CONTROLLER);r&&(h(this,de,(o=this.getRootNode())==null?void 0:o.getElementById(r)),(d=(n=s(this,de))==null?void 0:n.associateElement)==null||d.call(n,this))}disconnectedCallback(){var e,r;(r=(e=s(this,de))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,de,null)}};de=new WeakMap,Q.shadowRootOptions={mode:"open"},Q.getTemplateHTML=Cd,Q.getSlotTemplateHTML=Od;l.customElements.get("media-text-display")||l.customElements.define("media-text-display",Q);var Rs=Q;function Pd(i,t){return`
    <slot>${X(t.mediaDuration)}</slot>
  `}var St,ki=class extends Q{constructor(){var e;super();u(this,St,void 0);h(this,St,this.shadowRoot.querySelector("slot")),s(this,St).textContent=X((e=this.mediaDuration)!=null?e:0)}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_DURATION]}attributeChangedCallback(e,r,o){e===a.MEDIA_DURATION&&(s(this,St).textContent=X(+o)),super.attributeChangedCallback(e,r,o)}get mediaDuration(){return P(this,a.MEDIA_DURATION)}set mediaDuration(e){H(this,a.MEDIA_DURATION,e)}};St=new WeakMap,ki.getSlotTemplateHTML=Pd;l.customElements.get("media-duration-display")||l.customElements.define("media-duration-display",ki);var Ds=ki;var Ud={2:g("Network Error"),3:g("Decode Error"),4:g("Source Not Supported"),5:g("Encryption Error")},Nd={2:g("A network error caused the media download to fail."),3:g("A media error caused playback to be aborted. The media could be corrupt or your browser does not support this format."),4:g("An unsupported error occurred. The server or network failed, or your browser does not support this format."),5:g("The media is encrypted and there are no keys to decrypt it.")},Nr=i=>{var t,e;return i.code===1?null:{title:(t=Ud[i.code])!=null?t:`Error ${i.code}`,message:(e=Nd[i.code])!=null?e:i.message}};function Hd(i){let t=+i.mediaerrorcode;return`
    <style>
      :host {
        background: rgb(20 20 30 / .8);
      }

      #content {
        display: block;
        padding: 1.2em 1.5em;
      }

      h3,
      p {
        margin-block: 0 .3em;
      }
    </style>
    <slot name="error-${t}" id="content">
      ${ws({code:t,message:i.mediaerrormessage})}
    </slot>
  `}function Fd(i){return i.code&&Nr(i)!==null}function ws(i){var n;let{title:t,message:e}=(n=Nr(i))!=null?n:{},r=rr(i.code),o="";return t&&(o+=`<slot name="error-${r}-title"><h3>${rr(t)}</h3></slot>`),e&&(o+=`<slot name="error-${r}-message"><p>${rr(e)}</p></slot>`),o}var xs=[a.MEDIA_ERROR_CODE,a.MEDIA_ERROR_MESSAGE],Ri,It=class extends ie{constructor(){super(...arguments);u(this,Ri,null)}static get observedAttributes(){return[...super.observedAttributes,...xs]}formatErrorMessage(e){return this.constructor.formatErrorMessage(e)}attributeChangedCallback(e,r,o){var d;if(super.attributeChangedCallback(e,r,o),!xs.includes(e))return;let n=(d=this.mediaError)!=null?d:{code:this.mediaErrorCode,message:this.mediaErrorMessage};if(this.open=Fd(n),this.open&&(this.shadowRoot.querySelector("slot").name=`error-${this.mediaErrorCode}`,this.shadowRoot.querySelector("#content").innerHTML=this.formatErrorMessage(n),!this.hasAttribute("aria-label"))){let{title:c}=Nr(n);c&&this.setAttribute("aria-label",c)}}get mediaError(){return s(this,Ri)}set mediaError(e){h(this,Ri,e)}get mediaErrorCode(){return P(this,"mediaerrorcode")}set mediaErrorCode(e){H(this,"mediaerrorcode",e)}get mediaErrorMessage(){return D(this,"mediaerrormessage")}set mediaErrorMessage(e){R(this,"mediaerrormessage",e)}};Ri=new WeakMap,It.getSlotTemplateHTML=Hd,It.formatErrorMessage=ws;l.customElements.get("media-error-dialog")||l.customElements.define("media-error-dialog",It);var Cs=It;var $d=`<svg aria-hidden="true" viewBox="0 0 26 24">
  <path d="M16 3v2.5h3.5V9H22V3h-6ZM4 9h2.5V5.5H10V3H4v6Zm15.5 9.5H16V21h6v-6h-2.5v3.5ZM6.5 15H4v6h6v-2.5H6.5V15Z"/>
</svg>`,Bd=`<svg aria-hidden="true" viewBox="0 0 26 24">
  <path d="M18.5 6.5V3H16v6h6V6.5h-3.5ZM16 21h2.5v-3.5H22V15h-6v6ZM4 17.5h3.5V21H10v-6H4v2.5Zm3.5-11H4V9h6V3H7.5v3.5Z"/>
</svg>`;function Vd(i){return`
    <style>
      :host([${a.MEDIA_IS_FULLSCREEN}]) slot[name=icon] slot:not([name=exit]) {
        display: none !important;
      }

      
      :host(:not([${a.MEDIA_IS_FULLSCREEN}])) slot[name=icon] slot:not([name=enter]) {
        display: none !important;
      }

      :host([${a.MEDIA_IS_FULLSCREEN}]) slot[name=tooltip-enter],
      :host(:not([${a.MEDIA_IS_FULLSCREEN}])) slot[name=tooltip-exit] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="enter">${$d}</slot>
      <slot name="exit">${Bd}</slot>
    </slot>
  `}function Kd(){return`
    <slot name="tooltip-enter">${g("Enter fullscreen mode")}</slot>
    <slot name="tooltip-exit">${g("Exit fullscreen mode")}</slot>
  `}var Os=i=>{let t=i.mediaIsFullscreen?g("exit fullscreen mode"):g("enter fullscreen mode");i.setAttribute("aria-label",t)},Di,Mt=class extends U{constructor(){super(...arguments);u(this,Di,null)}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_IS_FULLSCREEN,a.MEDIA_FULLSCREEN_UNAVAILABLE]}connectedCallback(){super.connectedCallback(),Os(this)}attributeChangedCallback(e,r,o){super.attributeChangedCallback(e,r,o),e===a.MEDIA_IS_FULLSCREEN&&Os(this)}get mediaFullscreenUnavailable(){return D(this,a.MEDIA_FULLSCREEN_UNAVAILABLE)}set mediaFullscreenUnavailable(e){R(this,a.MEDIA_FULLSCREEN_UNAVAILABLE,e)}get mediaIsFullscreen(){return y(this,a.MEDIA_IS_FULLSCREEN)}set mediaIsFullscreen(e){S(this,a.MEDIA_IS_FULLSCREEN,e)}handleClick(e){h(this,Di,e);let r=s(this,Di)instanceof PointerEvent,o=this.mediaIsFullscreen?new l.CustomEvent(E.MEDIA_EXIT_FULLSCREEN_REQUEST,{composed:!0,bubbles:!0}):new l.CustomEvent(E.MEDIA_ENTER_FULLSCREEN_REQUEST,{composed:!0,bubbles:!0,detail:r});this.dispatchEvent(o)}};Di=new WeakMap,Mt.getSlotTemplateHTML=Vd,Mt.getTooltipContentHTML=Kd;l.customElements.get("media-fullscreen-button")||l.customElements.define("media-fullscreen-button",Mt);var Ps=Mt;var{MEDIA_TIME_IS_LIVE:Hr,MEDIA_PAUSED:xi}=a,{MEDIA_SEEK_TO_LIVE_REQUEST:Gd,MEDIA_PLAY_REQUEST:qd}=E,Wd='<svg viewBox="0 0 6 12" aria-hidden="true"><circle cx="3" cy="6" r="2"></circle></svg>';function Yd(i){return`
    <style>
      :host { --media-tooltip-display: none; }
      
      slot[name=indicator] > *,
      :host ::slotted([slot=indicator]) {
        
        min-width: auto;
        fill: var(--media-live-button-icon-color, rgb(140, 140, 140));
        color: var(--media-live-button-icon-color, rgb(140, 140, 140));
      }

      :host([${Hr}]:not([${xi}])) slot[name=indicator] > *,
      :host([${Hr}]:not([${xi}])) ::slotted([slot=indicator]) {
        fill: var(--media-live-button-indicator-color, rgb(255, 0, 0));
        color: var(--media-live-button-indicator-color, rgb(255, 0, 0));
      }

      :host([${Hr}]:not([${xi}])) {
        cursor: var(--media-cursor, not-allowed);
      }

      slot[name=text]{
        text-transform: uppercase;
      }

    </style>

    <slot name="indicator">${Wd}</slot>
    
    <slot name="spacer">&nbsp;</slot><slot name="text">${g("live")}</slot>
  `}var Us=i=>{var o;let t=i.mediaPaused||!i.mediaTimeIsLive,e=t?g("seek to live"):g("playing live");i.setAttribute("aria-label",e);let r=(o=i.shadowRoot)==null?void 0:o.querySelector('slot[name="text"]');r&&(r.textContent=g("live")),t?i.removeAttribute("aria-disabled"):i.setAttribute("aria-disabled","true")},wi=class extends U{static get observedAttributes(){return[...super.observedAttributes,Hr,xi]}connectedCallback(){super.connectedCallback(),Us(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),Us(this)}get mediaPaused(){return y(this,a.MEDIA_PAUSED)}set mediaPaused(t){S(this,a.MEDIA_PAUSED,t)}get mediaTimeIsLive(){return y(this,a.MEDIA_TIME_IS_LIVE)}set mediaTimeIsLive(t){S(this,a.MEDIA_TIME_IS_LIVE,t)}handleClick(){!this.mediaPaused&&this.mediaTimeIsLive||(this.dispatchEvent(new l.CustomEvent(Gd,{composed:!0,bubbles:!0})),this.hasAttribute(xi)&&this.dispatchEvent(new l.CustomEvent(qd,{composed:!0,bubbles:!0})))}};wi.getSlotTemplateHTML=Yd;l.customElements.get("media-live-button")||l.customElements.define("media-live-button",wi);var Ns=wi;var Fr={LOADING_DELAY:"loadingdelay",NO_AUTOHIDE:"noautohide"},Hs=500,Qd=`
<svg aria-hidden="true" viewBox="0 0 100 100">
  <path d="M73,50c0-12.7-10.3-23-23-23S27,37.3,27,50 M30.9,50c0-10.5,8.5-19.1,19.1-19.1S69.1,39.5,69.1,50">
    <animateTransform
       attributeName="transform"
       attributeType="XML"
       type="rotate"
       dur="1s"
       from="0 50 50"
       to="360 50 50"
       repeatCount="indefinite" />
  </path>
</svg>
`;function jd(i){return`
    <style>
      :host {
        display: var(--media-control-display, var(--media-loading-indicator-display, inline-block));
        vertical-align: middle;
        box-sizing: border-box;
        --_loading-indicator-delay: var(--media-loading-indicator-transition-delay, ${Hs}ms);
      }

      #status {
        color: rgba(0,0,0,0);
        width: 0px;
        height: 0px;
      }

      :host slot[name=icon] > *,
      :host ::slotted([slot=icon]) {
        opacity: var(--media-loading-indicator-opacity, 0);
        transition: opacity 0.15s;
      }

      :host([${a.MEDIA_LOADING}]:not([${a.MEDIA_PAUSED}])) slot[name=icon] > *,
      :host([${a.MEDIA_LOADING}]:not([${a.MEDIA_PAUSED}])) ::slotted([slot=icon]) {
        opacity: var(--media-loading-indicator-opacity, 1);
        transition: opacity 0.15s var(--_loading-indicator-delay);
      }

      :host #status {
        visibility: var(--media-loading-indicator-opacity, hidden);
        transition: visibility 0.15s;
      }

      :host([${a.MEDIA_LOADING}]:not([${a.MEDIA_PAUSED}])) #status {
        visibility: var(--media-loading-indicator-opacity, visible);
        transition: visibility 0.15s var(--_loading-indicator-delay);
      }

      svg, img, ::slotted(svg), ::slotted(img) {
        width: var(--media-loading-indicator-icon-width);
        height: var(--media-loading-indicator-icon-height, 100px);
        fill: var(--media-icon-color, var(--media-primary-color, rgb(238 238 238)));
        vertical-align: middle;
      }
    </style>

    <slot name="icon">${Qd}</slot>
    <div id="status" role="status" aria-live="polite">${g("media loading")}</div>
  `}var le,Ci,_t=class extends l.HTMLElement{constructor(){super();u(this,le,void 0);u(this,Ci,Hs);if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}}static get observedAttributes(){return[k.MEDIA_CONTROLLER,a.MEDIA_PAUSED,a.MEDIA_LOADING,Fr.LOADING_DELAY]}attributeChangedCallback(e,r,o){var n,d,c,m,b;e===Fr.LOADING_DELAY&&r!==o?this.loadingDelay=Number(o):e===k.MEDIA_CONTROLLER&&(r&&((d=(n=s(this,le))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,le,null)),o&&this.isConnected&&(h(this,le,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,le))==null?void 0:m.associateElement)==null||b.call(m,this)))}connectedCallback(){var r,o,n;let e=this.getAttribute(k.MEDIA_CONTROLLER);e&&(h(this,le,(r=this.getRootNode())==null?void 0:r.getElementById(e)),(n=(o=s(this,le))==null?void 0:o.associateElement)==null||n.call(o,this))}disconnectedCallback(){var e,r;(r=(e=s(this,le))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,le,null)}get loadingDelay(){return s(this,Ci)}set loadingDelay(e){h(this,Ci,e);let{style:r}=F(this.shadowRoot,":host");r.setProperty("--_loading-indicator-delay",`var(--media-loading-indicator-transition-delay, ${e}ms)`)}get mediaPaused(){return y(this,a.MEDIA_PAUSED)}set mediaPaused(e){S(this,a.MEDIA_PAUSED,e)}get mediaLoading(){return y(this,a.MEDIA_LOADING)}set mediaLoading(e){S(this,a.MEDIA_LOADING,e)}get mediaController(){return D(this,k.MEDIA_CONTROLLER)}set mediaController(e){R(this,k.MEDIA_CONTROLLER,e)}get noAutohide(){return y(this,Fr.NO_AUTOHIDE)}set noAutohide(e){S(this,Fr.NO_AUTOHIDE,e)}};le=new WeakMap,Ci=new WeakMap,_t.shadowRootOptions={mode:"open"},_t.getTemplateHTML=jd;l.customElements.get("media-loading-indicator")||l.customElements.define("media-loading-indicator",_t);var Fs=_t;var zd=`<svg aria-hidden="true" viewBox="0 0 24 24">
  <path d="M16.5 12A4.5 4.5 0 0 0 14 8v2.18l2.45 2.45a4.22 4.22 0 0 0 .05-.63Zm2.5 0a6.84 6.84 0 0 1-.54 2.64L20 16.15A8.8 8.8 0 0 0 21 12a9 9 0 0 0-7-8.77v2.06A7 7 0 0 1 19 12ZM4.27 3 3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25A6.92 6.92 0 0 1 14 18.7v2.06A9 9 0 0 0 17.69 19l2 2.05L21 19.73l-9-9L4.27 3ZM12 4 9.91 6.09 12 8.18V4Z"/>
</svg>`,$s=`<svg aria-hidden="true" viewBox="0 0 24 24">
  <path d="M3 9v6h4l5 5V4L7 9H3Zm13.5 3A4.5 4.5 0 0 0 14 8v8a4.47 4.47 0 0 0 2.5-4Z"/>
</svg>`,Zd=`<svg aria-hidden="true" viewBox="0 0 24 24">
  <path d="M3 9v6h4l5 5V4L7 9H3Zm13.5 3A4.5 4.5 0 0 0 14 8v8a4.47 4.47 0 0 0 2.5-4ZM14 3.23v2.06a7 7 0 0 1 0 13.42v2.06a9 9 0 0 0 0-17.54Z"/>
</svg>`;function Xd(i){return`
    <style>
      :host(:not([${a.MEDIA_VOLUME_LEVEL}])) slot[name=icon] slot:not([name=high]),
      :host([${a.MEDIA_VOLUME_LEVEL}=high]) slot[name=icon] slot:not([name=high]) {
        display: none !important;
      }

      :host([${a.MEDIA_VOLUME_LEVEL}=off]) slot[name=icon] slot:not([name=off]) {
        display: none !important;
      }

      :host([${a.MEDIA_VOLUME_LEVEL}=low]) slot[name=icon] slot:not([name=low]) {
        display: none !important;
      }

      :host([${a.MEDIA_VOLUME_LEVEL}=medium]) slot[name=icon] slot:not([name=medium]) {
        display: none !important;
      }

      :host(:not([${a.MEDIA_VOLUME_LEVEL}=off])) slot[name=tooltip-unmute],
      :host([${a.MEDIA_VOLUME_LEVEL}=off]) slot[name=tooltip-mute] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="off">${zd}</slot>
      <slot name="low">${$s}</slot>
      <slot name="medium">${$s}</slot>
      <slot name="high">${Zd}</slot>
    </slot>
  `}function Jd(){return`
    <slot name="tooltip-mute">${g("Mute")}</slot>
    <slot name="tooltip-unmute">${g("Unmute")}</slot>
  `}var Bs=i=>{let e=i.mediaVolumeLevel==="off"?g("unmute"):g("mute");i.setAttribute("aria-label",e)},Lt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_VOLUME_LEVEL]}connectedCallback(){super.connectedCallback(),Bs(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),t===a.MEDIA_VOLUME_LEVEL&&Bs(this)}get mediaVolumeLevel(){return D(this,a.MEDIA_VOLUME_LEVEL)}set mediaVolumeLevel(t){R(this,a.MEDIA_VOLUME_LEVEL,t)}handleClick(){let t=this.mediaVolumeLevel==="off"?E.MEDIA_UNMUTE_REQUEST:E.MEDIA_MUTE_REQUEST;this.dispatchEvent(new l.CustomEvent(t,{composed:!0,bubbles:!0}))}};Lt.getSlotTemplateHTML=Xd,Lt.getTooltipContentHTML=Jd;l.customElements.get("media-mute-button")||l.customElements.define("media-mute-button",Lt);var Vs=Lt;var Ks=`<svg aria-hidden="true" viewBox="0 0 28 24">
  <path d="M24 3H4a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h20a1 1 0 0 0 1-1V4a1 1 0 0 0-1-1Zm-1 16H5V5h18v14Zm-3-8h-7v5h7v-5Z"/>
</svg>`;function el(i){return`
    <style>
      :host([${a.MEDIA_IS_PIP}]) slot[name=icon] slot:not([name=exit]) {
        display: none !important;
      }

      :host(:not([${a.MEDIA_IS_PIP}])) slot[name=icon] slot:not([name=enter]) {
        display: none !important;
      }

      :host([${a.MEDIA_IS_PIP}]) slot[name=tooltip-enter],
      :host(:not([${a.MEDIA_IS_PIP}])) slot[name=tooltip-exit] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="enter">${Ks}</slot>
      <slot name="exit">${Ks}</slot>
    </slot>
  `}function tl(){return`
    <slot name="tooltip-enter">${g("Enter picture in picture mode")}</slot>
    <slot name="tooltip-exit">${g("Exit picture in picture mode")}</slot>
  `}var Gs=i=>{let t=i.mediaIsPip?g("exit picture in picture mode"):g("enter picture in picture mode");i.setAttribute("aria-label",t)},kt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_IS_PIP,a.MEDIA_PIP_UNAVAILABLE]}connectedCallback(){super.connectedCallback(),Gs(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),t===a.MEDIA_IS_PIP&&Gs(this)}get mediaPipUnavailable(){return D(this,a.MEDIA_PIP_UNAVAILABLE)}set mediaPipUnavailable(t){R(this,a.MEDIA_PIP_UNAVAILABLE,t)}get mediaIsPip(){return y(this,a.MEDIA_IS_PIP)}set mediaIsPip(t){S(this,a.MEDIA_IS_PIP,t)}handleClick(){let t=this.mediaIsPip?E.MEDIA_EXIT_PIP_REQUEST:E.MEDIA_ENTER_PIP_REQUEST;this.dispatchEvent(new l.CustomEvent(t,{composed:!0,bubbles:!0}))}};kt.getSlotTemplateHTML=el,kt.getTooltipContentHTML=tl;l.customElements.get("media-pip-button")||l.customElements.define("media-pip-button",kt);var qs=kt;var Mo={RATES:"rates"},il=[1,1.2,1.5,1.7,2],Oi=1;function _o(i){return Math.round(i*100)/100}function rl(i){return`
    <style>
      :host {
        min-width: 5ch;
        padding: var(--media-button-padding, var(--media-control-padding, 10px 5px));
      }
    </style>
    <slot name="icon">${i.mediaplaybackrate?_o(+i.mediaplaybackrate):Oi}x</slot>
  `}function ol(){return g("Playback rate")}var Ae,Rt=class extends U{constructor(){var e;super();u(this,Ae,new ct(this,Mo.RATES,{defaultValue:il}));this.container=this.shadowRoot.querySelector('slot[name="icon"]'),this.container.innerHTML=`${_o((e=this.mediaPlaybackRate)!=null?e:Oi)}x`}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_PLAYBACK_RATE,Mo.RATES]}attributeChangedCallback(e,r,o){if(super.attributeChangedCallback(e,r,o),e===Mo.RATES&&(s(this,Ae).value=o),e===a.MEDIA_PLAYBACK_RATE){let n=o?+o:Number.NaN,d=_o(Number.isNaN(n)?Oi:n);this.container.innerHTML=`${d}x`,this.setAttribute("aria-label",g("Playback rate {playbackRate}",{playbackRate:d}))}}get rates(){return s(this,Ae)}set rates(e){e?Array.isArray(e)?s(this,Ae).value=e.join(" "):typeof e=="string"&&(s(this,Ae).value=e):s(this,Ae).value=""}get mediaPlaybackRate(){return P(this,a.MEDIA_PLAYBACK_RATE,Oi)}set mediaPlaybackRate(e){H(this,a.MEDIA_PLAYBACK_RATE,e)}handleClick(){var n,d;let e=Array.from(s(this,Ae).values(),c=>+c).sort((c,m)=>c-m),r=(d=(n=e.find(c=>c>this.mediaPlaybackRate))!=null?n:e[0])!=null?d:Oi,o=new l.CustomEvent(E.MEDIA_PLAYBACK_RATE_REQUEST,{composed:!0,bubbles:!0,detail:r});this.dispatchEvent(o)}};Ae=new WeakMap,Rt.getSlotTemplateHTML=rl,Rt.getTooltipContentHTML=ol;l.customElements.get("media-playback-rate-button")||l.customElements.define("media-playback-rate-button",Rt);var Ws=Rt;var nl=`<svg aria-hidden="true" viewBox="0 0 24 24">
  <path d="m6 21 15-9L6 3v18Z"/>
</svg>`,sl=`<svg aria-hidden="true" viewBox="0 0 24 24">
  <path d="M6 20h4V4H6v16Zm8-16v16h4V4h-4Z"/>
</svg>`;function al(i){return`
    <style>
      :host([${a.MEDIA_PAUSED}]) slot[name=pause],
      :host(:not([${a.MEDIA_PAUSED}])) slot[name=play] {
        display: none !important;
      }

      :host([${a.MEDIA_PAUSED}]) slot[name=tooltip-pause],
      :host(:not([${a.MEDIA_PAUSED}])) slot[name=tooltip-play] {
        display: none;
      }
    </style>

    <slot name="icon">
      <slot name="play">${nl}</slot>
      <slot name="pause">${sl}</slot>
    </slot>
  `}function dl(){return`
    <slot name="tooltip-play">${g("Play")}</slot>
    <slot name="tooltip-pause">${g("Pause")}</slot>
  `}var Ys=i=>{let t=i.mediaPaused?g("play"):g("pause");i.setAttribute("aria-label",t)},Dt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_PAUSED,a.MEDIA_ENDED]}connectedCallback(){super.connectedCallback(),Ys(this)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),(t===a.MEDIA_PAUSED||t===a.MEDIA_LANG)&&Ys(this)}get mediaPaused(){return y(this,a.MEDIA_PAUSED)}set mediaPaused(t){S(this,a.MEDIA_PAUSED,t)}handleClick(){let t=this.mediaPaused?E.MEDIA_PLAY_REQUEST:E.MEDIA_PAUSE_REQUEST;this.dispatchEvent(new l.CustomEvent(t,{composed:!0,bubbles:!0}))}};Dt.getSlotTemplateHTML=al,Dt.getTooltipContentHTML=dl;l.customElements.get("media-play-button")||l.customElements.define("media-play-button",Dt);var Qs=Dt;var Ee={PLACEHOLDER_SRC:"placeholdersrc",SRC:"src"};function ll(i){return`
    <style>
      :host {
        pointer-events: none;
        display: var(--media-poster-image-display, inline-block);
        box-sizing: border-box;
      }

      img {
        max-width: 100%;
        max-height: 100%;
        min-width: 100%;
        min-height: 100%;
        background-repeat: no-repeat;
        background-position: var(--media-poster-image-background-position, var(--media-object-position, center));
        background-size: var(--media-poster-image-background-size, var(--media-object-fit, contain));
        object-fit: var(--media-object-fit, contain);
        object-position: var(--media-object-position, center);
      }
    </style>

    <img part="poster img" aria-hidden="true" id="image"/>
  `}var cl=i=>{i.style.removeProperty("background-image")},ul=(i,t)=>{i.style["background-image"]=`url('${t}')`},xt=class extends l.HTMLElement{static get observedAttributes(){return[Ee.PLACEHOLDER_SRC,Ee.SRC]}constructor(){if(super(),!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let t=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(t)}this.image=this.shadowRoot.querySelector("#image")}attributeChangedCallback(t,e,r){t===Ee.SRC&&(r==null?this.image.removeAttribute(Ee.SRC):this.image.setAttribute(Ee.SRC,r)),t===Ee.PLACEHOLDER_SRC&&(r==null?cl(this.image):ul(this.image,r))}get placeholderSrc(){return D(this,Ee.PLACEHOLDER_SRC)}set placeholderSrc(t){R(this,Ee.SRC,t)}get src(){return D(this,Ee.SRC)}set src(t){R(this,Ee.SRC,t)}};xt.shadowRootOptions={mode:"open"},xt.getTemplateHTML=ll;l.customElements.get("media-poster-image")||l.customElements.define("media-poster-image",xt);var js=xt;var Pi,$r=class extends Q{constructor(){super();u(this,Pi,void 0);h(this,Pi,this.shadowRoot.querySelector("slot"))}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_PREVIEW_CHAPTER,a.MEDIA_LANG]}attributeChangedCallback(e,r,o){if(super.attributeChangedCallback(e,r,o),(e===a.MEDIA_PREVIEW_CHAPTER||e===a.MEDIA_LANG)&&o!==r&&o!=null)if(s(this,Pi).textContent=o,o!==""){let n=g("chapter: {chapterName}",{chapterName:o});this.setAttribute("aria-valuetext",n)}else this.removeAttribute("aria-valuetext")}get mediaPreviewChapter(){return D(this,a.MEDIA_PREVIEW_CHAPTER)}set mediaPreviewChapter(e){R(this,a.MEDIA_PREVIEW_CHAPTER,e)}};Pi=new WeakMap;l.customElements.get("media-preview-chapter-display")||l.customElements.define("media-preview-chapter-display",$r);var zs=$r;function ml(i){return`
    <style>
      :host {
        box-sizing: border-box;
        display: var(--media-control-display, var(--media-preview-thumbnail-display, inline-block));
        overflow: hidden;
      }

      img {
        display: none;
        position: relative;
      }
    </style>
    <img crossorigin loading="eager" decoding="async">
  `}var ce,wt=class extends l.HTMLElement{constructor(){super();u(this,ce,void 0);if(!this.shadowRoot){this.attachShadow(this.constructor.shadowRootOptions);let e=B(this.attributes);this.shadowRoot.innerHTML=this.constructor.getTemplateHTML(e)}}static get observedAttributes(){return[k.MEDIA_CONTROLLER,a.MEDIA_PREVIEW_IMAGE,a.MEDIA_PREVIEW_COORDS]}connectedCallback(){var r,o,n;let e=this.getAttribute(k.MEDIA_CONTROLLER);e&&(h(this,ce,(r=this.getRootNode())==null?void 0:r.getElementById(e)),(n=(o=s(this,ce))==null?void 0:o.associateElement)==null||n.call(o,this))}disconnectedCallback(){var e,r;(r=(e=s(this,ce))==null?void 0:e.unassociateElement)==null||r.call(e,this),h(this,ce,null)}attributeChangedCallback(e,r,o){var n,d,c,m,b;[a.MEDIA_PREVIEW_IMAGE,a.MEDIA_PREVIEW_COORDS].includes(e)&&this.update(),e===k.MEDIA_CONTROLLER&&(r&&((d=(n=s(this,ce))==null?void 0:n.unassociateElement)==null||d.call(n,this),h(this,ce,null)),o&&this.isConnected&&(h(this,ce,(c=this.getRootNode())==null?void 0:c.getElementById(o)),(b=(m=s(this,ce))==null?void 0:m.associateElement)==null||b.call(m,this)))}get mediaPreviewImage(){return D(this,a.MEDIA_PREVIEW_IMAGE)}set mediaPreviewImage(e){R(this,a.MEDIA_PREVIEW_IMAGE,e)}get mediaPreviewCoords(){let e=this.getAttribute(a.MEDIA_PREVIEW_COORDS);if(e)return e.split(/\s+/).map(r=>+r)}set mediaPreviewCoords(e){if(!e){this.removeAttribute(a.MEDIA_PREVIEW_COORDS);return}this.setAttribute(a.MEDIA_PREVIEW_COORDS,e.join(" "))}update(){let e=this.mediaPreviewCoords,r=this.mediaPreviewImage;if(!(e&&r))return;let[o,n,d,c]=e,m=r.split("#")[0],b=getComputedStyle(this),{maxWidth:A,maxHeight:M,minWidth:T,minHeight:v}=b,x=b.getPropertyValue("--media-preview-thumbnail-object-fit").trim()||"contain",_,L;if(x==="fill"){let ge=parseInt(A)/d,be=parseInt(M)/c,Qt=parseInt(T)/d,Oe=parseInt(v)/c;_=ge<1?ge:Math.max(ge,Qt),L=be<1?be:Math.max(be,Oe)}else{let ge=Math.min(parseInt(A)/d,parseInt(M)/c),be=Math.max(parseInt(T)/d,parseInt(v)/c),Oe=ge<1?ge:be>1?be:1;_=Oe,L=Oe}let{style:C}=F(this.shadowRoot,":host"),O=F(this.shadowRoot,"img").style,ee=this.shadowRoot.querySelector("img"),Yt=Math.min(_,L)<1?"min":"max";C.setProperty(`${Yt}-width`,"initial","important"),C.setProperty(`${Yt}-height`,"initial","important"),C.width=`${d*_}px`,C.height=`${c*L}px`;let it=()=>{O.width=`${this.imgWidth*_}px`,O.height=`${this.imgHeight*L}px`,O.display="block"};ee.src!==m&&(ee.onload=()=>{this.imgWidth=ee.naturalWidth,this.imgHeight=ee.naturalHeight,it(),ee.onload=null},ee.src=m,it()),it(),O.transform=`translate(-${o*_}px, -${n*L}px)`}};ce=new WeakMap,wt.shadowRootOptions={mode:"open"},wt.getTemplateHTML=ml;l.customElements.get("media-preview-thumbnail")||l.customElements.define("media-preview-thumbnail",wt);var Ui=wt;var Ct,Br=class extends Q{constructor(){super();u(this,Ct,void 0);h(this,Ct,this.shadowRoot.querySelector("slot")),s(this,Ct).textContent=X(0)}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_PREVIEW_TIME]}attributeChangedCallback(e,r,o){super.attributeChangedCallback(e,r,o),e===a.MEDIA_PREVIEW_TIME&&o!=null&&(s(this,Ct).textContent=X(parseFloat(o)))}get mediaPreviewTime(){return P(this,a.MEDIA_PREVIEW_TIME)}set mediaPreviewTime(e){H(this,a.MEDIA_PREVIEW_TIME,e)}};Ct=new WeakMap;l.customElements.get("media-preview-time-display")||l.customElements.define("media-preview-time-display",Br);var Zs=Br;var Ot={SEEK_OFFSET:"seekoffset"},Lo=30,hl=i=>`
  <svg aria-hidden="true" viewBox="0 0 20 24">
    <defs>
      <style>.text{font-size:8px;font-family:Arial-BoldMT, Arial;font-weight:700;}</style>
    </defs>
    <text class="text value" transform="translate(2.18 19.87)">${i}</text>
    <path d="M10 6V3L4.37 7 10 10.94V8a5.54 5.54 0 0 1 1.9 10.48v2.12A7.5 7.5 0 0 0 10 6Z"/>
  </svg>`;function pl(i,t){return`
    <slot name="icon">${hl(t.seekOffset)}</slot>
  `}var El=(i,t)=>{i.setAttribute("aria-label",g("seek back {seekOffset} seconds",{seekOffset:t}))};function gl(){return g("Seek backward")}var bl=0,Pt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_CURRENT_TIME,Ot.SEEK_OFFSET]}connectedCallback(){super.connectedCallback(),this.seekOffset=P(this,Ot.SEEK_OFFSET,Lo)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),El(this,this.seekOffset),t===Ot.SEEK_OFFSET&&(this.seekOffset=P(this,Ot.SEEK_OFFSET,Lo))}get seekOffset(){return P(this,Ot.SEEK_OFFSET,Lo)}set seekOffset(t){H(this,Ot.SEEK_OFFSET,t),this.setAttribute("aria-label",g("seek back {seekOffset} seconds",{seekOffset:this.seekOffset})),cr(ur(this,"icon"),this.seekOffset)}get mediaCurrentTime(){return P(this,a.MEDIA_CURRENT_TIME,bl)}set mediaCurrentTime(t){H(this,a.MEDIA_CURRENT_TIME,t)}handleClick(){let t=Math.max(this.mediaCurrentTime-this.seekOffset,0),e=new l.CustomEvent(E.MEDIA_SEEK_REQUEST,{composed:!0,bubbles:!0,detail:t});this.dispatchEvent(e)}};Pt.getSlotTemplateHTML=pl,Pt.getTooltipContentHTML=gl;l.customElements.get("media-seek-backward-button")||l.customElements.define("media-seek-backward-button",Pt);var Xs=Pt;var Ut={SEEK_OFFSET:"seekoffset"},ko=30,fl=i=>`
  <svg aria-hidden="true" viewBox="0 0 20 24">
    <defs>
      <style>.text{font-size:8px;font-family:Arial-BoldMT, Arial;font-weight:700;}</style>
    </defs>
    <text class="text value" transform="translate(8.9 19.87)">${i}</text>
    <path d="M10 6V3l5.61 4L10 10.94V8a5.54 5.54 0 0 0-1.9 10.48v2.12A7.5 7.5 0 0 1 10 6Z"/>
  </svg>`;function vl(i,t){return`
    <slot name="icon">${fl(t.seekOffset)}</slot>
  `}var Tl=(i,t)=>{i.setAttribute("aria-label",g("seek forward {seekOffset} seconds",{seekOffset:t}))};function Al(){return g("Seek forward")}var yl=0,Nt=class extends U{static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_CURRENT_TIME,Ut.SEEK_OFFSET]}connectedCallback(){super.connectedCallback(),this.seekOffset=P(this,Ut.SEEK_OFFSET,ko)}attributeChangedCallback(t,e,r){super.attributeChangedCallback(t,e,r),Tl(this,this.seekOffset),t===Ut.SEEK_OFFSET&&(this.seekOffset=P(this,Ut.SEEK_OFFSET,ko))}get seekOffset(){return P(this,Ut.SEEK_OFFSET,ko)}set seekOffset(t){H(this,Ut.SEEK_OFFSET,t),this.setAttribute("aria-label",g("seek forward {seekOffset} seconds",{seekOffset:this.seekOffset})),cr(ur(this,"icon"),this.seekOffset)}get mediaCurrentTime(){return P(this,a.MEDIA_CURRENT_TIME,yl)}set mediaCurrentTime(t){H(this,a.MEDIA_CURRENT_TIME,t)}handleClick(){let t=this.mediaCurrentTime+this.seekOffset,e=new l.CustomEvent(E.MEDIA_SEEK_REQUEST,{composed:!0,bubbles:!0,detail:t});this.dispatchEvent(e)}};Nt.getSlotTemplateHTML=vl,Nt.getTooltipContentHTML=Al;l.customElements.get("media-seek-forward-button")||l.customElements.define("media-seek-forward-button",Nt);var Js=Nt;var we={REMAINING:"remaining",SHOW_DURATION:"showduration",NO_TOGGLE:"notoggle"},ea=[...Object.values(we),a.MEDIA_CURRENT_TIME,a.MEDIA_DURATION,a.MEDIA_SEEKABLE],ta=["Enter"," "],Sl="&nbsp;/&nbsp;",Ro=(i,{timesSep:t=Sl}={})=>{var d,c;let e=(d=i.mediaCurrentTime)!=null?d:0,[,r]=(c=i.mediaSeekable)!=null?c:[],o=0;Number.isFinite(i.mediaDuration)?o=i.mediaDuration:Number.isFinite(r)&&(o=r);let n=i.remaining?X(0-(o-e)):X(e);return i.showDuration?`${n}${t}${X(o)}`:n},Il=i=>{var c;let t=i.mediaCurrentTime,[,e]=(c=i.mediaSeekable)!=null?c:[],r=null;if(Number.isFinite(i.mediaDuration)?r=i.mediaDuration:Number.isFinite(e)&&(r=e),t==null||r===null){i.setAttribute("aria-description",g("video not loaded, unknown time."));return}let o=i.remaining?Ie(0-(r-t)):Ie(t);if(!i.showDuration){i.setAttribute("aria-description",o);return}let n=Ie(r),d=g("{currentTime} of {totalTime}",{currentTime:o,totalTime:n});i.setAttribute("aria-description",d)};function Ml(i,t){return`
    <slot>${Ro(t)}</slot>
  `}var _l=i=>{i.setAttribute("aria-label",g("playback time"))},je,ue,Hi,Kr,ia,Fi,Do,Ht,Vr,$i,xo,Ni=class extends Q{constructor(){super();u(this,Kr);u(this,Fi);u(this,Ht);u(this,$i);u(this,je,void 0);u(this,ue,null);u(this,Hi,e=>{let{metaKey:r,altKey:o,key:n}=e;if(r||o||!ta.includes(n)){this.removeEventListener("keyup",s(this,ue));return}this.addEventListener("keyup",s(this,ue))});h(this,je,this.shadowRoot.querySelector("slot")),s(this,je).innerHTML=`${Ro(this)}`}static get observedAttributes(){return[...super.observedAttributes,...ea,"disabled"]}connectedCallback(){let{style:e}=F(this.shadowRoot,":host(:hover:not([notoggle]))");e.setProperty("cursor","var(--media-cursor, pointer)"),e.setProperty("background","var(--media-control-hover-background, rgba(50 50 70 / .7))"),this.setAttribute("aria-label",g("playback time")),f(this,Ht,Vr).call(this),super.connectedCallback()}toggleTimeDisplay(){this.noToggle||(this.hasAttribute("remaining")?this.removeAttribute("remaining"):this.setAttribute("remaining",""))}disconnectedCallback(){this.disable(),f(this,Fi,Do).call(this),super.disconnectedCallback()}attributeChangedCallback(e,r,o){_l(this),ea.includes(e)?this.update():e==="disabled"&&o!==r?o==null?f(this,Ht,Vr).call(this):f(this,$i,xo).call(this):e===we.NO_TOGGLE&&o!==r&&(this.noToggle?f(this,$i,xo).call(this):f(this,Ht,Vr).call(this)),super.attributeChangedCallback(e,r,o)}enable(){this.noToggle||(this.tabIndex=0)}disable(){this.tabIndex=-1}get remaining(){return y(this,we.REMAINING)}set remaining(e){S(this,we.REMAINING,e)}get showDuration(){return y(this,we.SHOW_DURATION)}set showDuration(e){S(this,we.SHOW_DURATION,e)}get noToggle(){return y(this,we.NO_TOGGLE)}set noToggle(e){S(this,we.NO_TOGGLE,e)}get mediaDuration(){return P(this,a.MEDIA_DURATION)}set mediaDuration(e){H(this,a.MEDIA_DURATION,e)}get mediaCurrentTime(){return P(this,a.MEDIA_CURRENT_TIME)}set mediaCurrentTime(e){H(this,a.MEDIA_CURRENT_TIME,e)}get mediaSeekable(){let e=this.getAttribute(a.MEDIA_SEEKABLE);if(e)return e.split(":").map(r=>+r)}set mediaSeekable(e){if(e==null){this.removeAttribute(a.MEDIA_SEEKABLE);return}this.setAttribute(a.MEDIA_SEEKABLE,e.join(":"))}update(){let e=Ro(this);Il(this),e!==s(this,je).innerHTML&&(s(this,je).innerHTML=e)}};je=new WeakMap,ue=new WeakMap,Hi=new WeakMap,Kr=new WeakSet,ia=function(){s(this,ue)||(h(this,ue,e=>{let{key:r}=e;if(!ta.includes(r)){this.removeEventListener("keyup",s(this,ue));return}this.toggleTimeDisplay()}),this.addEventListener("keydown",s(this,Hi)),this.addEventListener("click",this.toggleTimeDisplay))},Fi=new WeakSet,Do=function(){s(this,ue)&&(this.removeEventListener("keyup",s(this,ue)),this.removeEventListener("keydown",s(this,Hi)),this.removeEventListener("click",this.toggleTimeDisplay),h(this,ue,null))},Ht=new WeakSet,Vr=function(){!this.noToggle&&!this.hasAttribute("disabled")&&(this.setAttribute("role","button"),this.enable(),f(this,Kr,ia).call(this))},$i=new WeakSet,xo=function(){this.removeAttribute("role"),this.disable(),f(this,Fi,Do).call(this)},Ni.getSlotTemplateHTML=Ml;l.customElements.get("media-time-display")||l.customElements.define("media-time-display",Ni);var ra=Ni;var ze,Bi,Ze,Ft,Vi,Ki,Gi,Xe,Ce,qi,Gr=class{constructor(t,e,r){u(this,ze,void 0);u(this,Bi,void 0);u(this,Ze,void 0);u(this,Ft,void 0);u(this,Vi,void 0);u(this,Ki,void 0);u(this,Gi,void 0);u(this,Xe,void 0);u(this,Ce,0);u(this,qi,(t=performance.now())=>{h(this,Ce,requestAnimationFrame(s(this,qi))),h(this,Ft,performance.now()-s(this,Ze));let e=1e3/this.fps;if(s(this,Ft)>e){h(this,Ze,t-s(this,Ft)%e);let r=1e3/((t-s(this,Bi))/++ln(this,Vi)._),o=(t-s(this,Ki))/1e3/this.duration,n=s(this,Gi)+o*this.playbackRate;n-s(this,ze).valueAsNumber>0?h(this,Xe,this.playbackRate/this.duration/r):(h(this,Xe,.995*s(this,Xe)),n=s(this,ze).valueAsNumber+s(this,Xe)),this.callback(n)}});h(this,ze,t),this.callback=e,this.fps=r}start(){s(this,Ce)===0&&(h(this,Ze,performance.now()),h(this,Bi,s(this,Ze)),h(this,Vi,0),s(this,qi).call(this))}stop(){s(this,Ce)!==0&&(cancelAnimationFrame(s(this,Ce)),h(this,Ce,0))}update({start:t,duration:e,playbackRate:r}){let o=t-s(this,ze).valueAsNumber,n=Math.abs(e-this.duration);(o>0||o<-.5/e||n>=.5)&&this.callback(t),h(this,Gi,t),h(this,Ki,performance.now()),this.duration=e,this.playbackRate=r}};ze=new WeakMap,Bi=new WeakMap,Ze=new WeakMap,Ft=new WeakMap,Vi=new WeakMap,Ki=new WeakMap,Gi=new WeakMap,Xe=new WeakMap,Ce=new WeakMap,qi=new WeakMap;var Ll=i=>{let t=i.range,e=Ie(+oa(i)),r=Ie(+i.mediaSeekableEnd),o=e&&r?g("{currentTime} of {totalTime}",{currentTime:e,totalTime:r}):g("video not loaded, unknown time.");t.setAttribute("aria-valuetext",o)};function kl(i){return`
    <style>
      :host {
        --media-box-border-radius: 4px;
        --media-box-padding-left: 10px;
        --media-box-padding-right: 10px;
        --media-preview-border-radius: var(--media-box-border-radius);
        --media-box-arrow-offset: var(--media-box-border-radius);
        --_control-background: var(--media-control-background, var(--media-secondary-color, rgb(20 20 30 / .7)));
        --_preview-background: var(--media-preview-background, var(--_control-background));

        
        contain: layout;
      }

      #buffered {
        background: var(--media-time-range-buffered-color, rgb(255 255 255 / .4));
        position: absolute;
        height: 100%;
        will-change: width;
      }

      #preview-rail,
      #current-rail {
        width: 100%;
        position: absolute;
        left: 0;
        bottom: 100%;
        pointer-events: none;
        will-change: transform;
      }

      [part~="box"] {
        width: min-content;
        
        position: absolute;
        bottom: 100%;
        flex-direction: column;
        align-items: center;
        transform: translateX(-50%);
      }

      [part~="current-box"] {
        display: var(--media-current-box-display, var(--media-box-display, flex));
        margin: var(--media-current-box-margin, var(--media-box-margin, 0 0 5px));
        visibility: hidden;
      }

      [part~="preview-box"] {
        display: var(--media-preview-box-display, var(--media-box-display, flex));
        margin: var(--media-preview-box-margin, var(--media-box-margin, 0 0 5px));
        transition-property: var(--media-preview-transition-property, visibility, opacity);
        transition-duration: var(--media-preview-transition-duration-out, .25s);
        transition-delay: var(--media-preview-transition-delay-out, 0s);
        visibility: hidden;
        opacity: 0;
      }

      :host(:is([${a.MEDIA_PREVIEW_IMAGE}], [${a.MEDIA_PREVIEW_TIME}])[dragging]) [part~="preview-box"] {
        transition-duration: var(--media-preview-transition-duration-in, .5s);
        transition-delay: var(--media-preview-transition-delay-in, .25s);
        visibility: visible;
        opacity: 1;
      }

      @media (hover: hover) {
        :host(:is([${a.MEDIA_PREVIEW_IMAGE}], [${a.MEDIA_PREVIEW_TIME}]):hover) [part~="preview-box"] {
          transition-duration: var(--media-preview-transition-duration-in, .5s);
          transition-delay: var(--media-preview-transition-delay-in, .25s);
          visibility: visible;
          opacity: 1;
        }
      }

      media-preview-thumbnail,
      ::slotted(media-preview-thumbnail) {
        visibility: hidden;
        
        transition: visibility 0s .25s;
        transition-delay: calc(var(--media-preview-transition-delay-out, 0s) + var(--media-preview-transition-duration-out, .25s));
        background: var(--media-preview-thumbnail-background, var(--_preview-background));
        box-shadow: var(--media-preview-thumbnail-box-shadow, 0 0 4px rgb(0 0 0 / .2));
        max-width: var(--media-preview-thumbnail-max-width, 180px);
        max-height: var(--media-preview-thumbnail-max-height, 160px);
        min-width: var(--media-preview-thumbnail-min-width, 120px);
        min-height: var(--media-preview-thumbnail-min-height, 80px);
        border: var(--media-preview-thumbnail-border);
        border-radius: var(--media-preview-thumbnail-border-radius,
          var(--media-preview-border-radius) var(--media-preview-border-radius) 0 0);
      }

      :host([${a.MEDIA_PREVIEW_IMAGE}][dragging]) media-preview-thumbnail,
      :host([${a.MEDIA_PREVIEW_IMAGE}][dragging]) ::slotted(media-preview-thumbnail) {
        transition-delay: var(--media-preview-transition-delay-in, .25s);
        visibility: visible;
      }

      @media (hover: hover) {
        :host([${a.MEDIA_PREVIEW_IMAGE}]:hover) media-preview-thumbnail,
        :host([${a.MEDIA_PREVIEW_IMAGE}]:hover) ::slotted(media-preview-thumbnail) {
          transition-delay: var(--media-preview-transition-delay-in, .25s);
          visibility: visible;
        }

        :host([${a.MEDIA_PREVIEW_TIME}]:hover) {
          --media-time-range-hover-display: block;
        }
      }

      media-preview-chapter-display,
      ::slotted(media-preview-chapter-display) {
        font-size: var(--media-font-size, 13px);
        line-height: 17px;
        min-width: 0;
        visibility: hidden;
        
        transition: min-width 0s, border-radius 0s, margin 0s, padding 0s, visibility 0s;
        transition-delay: calc(var(--media-preview-transition-delay-out, 0s) + var(--media-preview-transition-duration-out, .25s));
        background: var(--media-preview-chapter-background, var(--_preview-background));
        border-radius: var(--media-preview-chapter-border-radius,
          var(--media-preview-border-radius) var(--media-preview-border-radius)
          var(--media-preview-border-radius) var(--media-preview-border-radius));
        padding: var(--media-preview-chapter-padding, 3.5px 9px);
        margin: var(--media-preview-chapter-margin, 0 0 5px);
        text-shadow: var(--media-preview-chapter-text-shadow, 0 0 4px rgb(0 0 0 / .75));
      }

      :host([${a.MEDIA_PREVIEW_IMAGE}]) media-preview-chapter-display,
      :host([${a.MEDIA_PREVIEW_IMAGE}]) ::slotted(media-preview-chapter-display) {
        transition-delay: var(--media-preview-transition-delay-in, .25s);
        border-radius: var(--media-preview-chapter-border-radius, 0);
        padding: var(--media-preview-chapter-padding, 3.5px 9px 0);
        margin: var(--media-preview-chapter-margin, 0);
        min-width: 100%;
      }

      media-preview-chapter-display[${a.MEDIA_PREVIEW_CHAPTER}],
      ::slotted(media-preview-chapter-display[${a.MEDIA_PREVIEW_CHAPTER}]) {
        visibility: visible;
      }

      media-preview-chapter-display:not([aria-valuetext]),
      ::slotted(media-preview-chapter-display:not([aria-valuetext])) {
        display: none;
      }

      media-preview-time-display,
      ::slotted(media-preview-time-display),
      media-time-display,
      ::slotted(media-time-display) {
        font-size: var(--media-font-size, 13px);
        line-height: 17px;
        min-width: 0;
        
        transition: min-width 0s, border-radius 0s;
        transition-delay: calc(var(--media-preview-transition-delay-out, 0s) + var(--media-preview-transition-duration-out, .25s));
        background: var(--media-preview-time-background, var(--_preview-background));
        border-radius: var(--media-preview-time-border-radius,
          var(--media-preview-border-radius) var(--media-preview-border-radius)
          var(--media-preview-border-radius) var(--media-preview-border-radius));
        padding: var(--media-preview-time-padding, 3.5px 9px);
        margin: var(--media-preview-time-margin, 0);
        text-shadow: var(--media-preview-time-text-shadow, 0 0 4px rgb(0 0 0 / .75));
        transform: translateX(min(
          max(calc(50% - var(--_box-width) / 2),
          calc(var(--_box-shift, 0))),
          calc(var(--_box-width) / 2 - 50%)
        ));
      }

      :host([${a.MEDIA_PREVIEW_IMAGE}]) media-preview-time-display,
      :host([${a.MEDIA_PREVIEW_IMAGE}]) ::slotted(media-preview-time-display) {
        transition-delay: var(--media-preview-transition-delay-in, .25s);
        border-radius: var(--media-preview-time-border-radius,
          0 0 var(--media-preview-border-radius) var(--media-preview-border-radius));
        min-width: 100%;
      }

      :host([${a.MEDIA_PREVIEW_TIME}]:hover) {
        --media-time-range-hover-display: block;
      }

      [part~="arrow"],
      ::slotted([part~="arrow"]) {
        display: var(--media-box-arrow-display, inline-block);
        transform: translateX(min(
          max(calc(50% - var(--_box-width) / 2 + var(--media-box-arrow-offset)),
          calc(var(--_box-shift, 0))),
          calc(var(--_box-width) / 2 - 50% - var(--media-box-arrow-offset))
        ));
        
        border-color: transparent;
        border-top-color: var(--media-box-arrow-background, var(--_control-background));
        border-width: var(--media-box-arrow-border-width,
          var(--media-box-arrow-height, 5px) var(--media-box-arrow-width, 6px) 0);
        border-style: solid;
        justify-content: center;
        height: 0;
      }
    </style>
    <div id="preview-rail">
      <slot name="preview" part="box preview-box">
        <media-preview-thumbnail>
          <template shadowrootmode="${Ui.shadowRootOptions.mode}">
            ${Ui.getTemplateHTML({})}
          </template>
        </media-preview-thumbnail>
        <media-preview-chapter-display></media-preview-chapter-display>
        <media-preview-time-display></media-preview-time-display>
        <slot name="preview-arrow"><div part="arrow"></div></slot>
      </slot>
    </div>
    <div id="current-rail">
      <slot name="current" part="box current-box">
        
      </slot>
    </div>
  `}var qr=(i,t=i.mediaCurrentTime)=>{let e=Number.isFinite(i.mediaSeekableStart)?i.mediaSeekableStart:0,r=Number.isFinite(i.mediaDuration)?i.mediaDuration:i.mediaSeekableEnd;if(Number.isNaN(r))return 0;let o=(t-e)/(r-e);return Math.max(0,Math.min(o,1))},oa=(i,t=i.range.valueAsNumber)=>{let e=Number.isFinite(i.mediaSeekableStart)?i.mediaSeekableStart:0,r=Number.isFinite(i.mediaDuration)?i.mediaDuration:i.mediaSeekableEnd;return Number.isNaN(r)?0:t*(r-e)+e},Je,ye,Wi,Bt,Yi,Qi,Vt,Kt,et,tt,Gt,Yr,na,Qr,ji,wo,zi,Co,Zi,Oo,jr,sa,qt,Wr,zr,aa,$t=class extends se{constructor(){super();u(this,Yr);u(this,ji);u(this,zi);u(this,Zi);u(this,jr);u(this,qt);u(this,zr);u(this,Je,null);u(this,ye,void 0);u(this,Wi,void 0);u(this,Bt,void 0);u(this,Yi,void 0);u(this,Qi,void 0);u(this,Vt,void 0);u(this,Kt,void 0);u(this,et,void 0);u(this,tt,void 0);u(this,Gt,()=>{f(this,Yr,na).call(this)?s(this,ye).start():s(this,ye).stop()});u(this,Qr,e=>{this.dragging||(ot(e)&&(this.range.valueAsNumber=e),s(this,tt)||this.updateBar())});this.shadowRoot.querySelector("#track").insertAdjacentHTML("afterbegin",'<div id="buffered" part="buffered"></div>'),h(this,Wi,this.shadowRoot.querySelectorAll('[part~="box"]')),h(this,Yi,this.shadowRoot.querySelector('[part~="preview-box"]')),h(this,Qi,this.shadowRoot.querySelector('[part~="current-box"]'));let r=getComputedStyle(this);h(this,Vt,parseInt(r.getPropertyValue("--media-box-padding-left"))),h(this,Kt,parseInt(r.getPropertyValue("--media-box-padding-right"))),h(this,ye,new Gr(this.range,s(this,Qr),60))}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_PAUSED,a.MEDIA_DURATION,a.MEDIA_SEEKABLE,a.MEDIA_CURRENT_TIME,a.MEDIA_PREVIEW_IMAGE,a.MEDIA_PREVIEW_TIME,a.MEDIA_PREVIEW_CHAPTER,a.MEDIA_BUFFERED,a.MEDIA_PLAYBACK_RATE,a.MEDIA_LOADING,a.MEDIA_ENDED]}connectedCallback(){var e;super.connectedCallback(),this.range.setAttribute("aria-label",g("seek")),s(this,Gt).call(this),h(this,Je,this.getRootNode()),(e=s(this,Je))==null||e.addEventListener("transitionstart",this)}disconnectedCallback(){var e;super.disconnectedCallback(),s(this,ye).stop(),(e=s(this,Je))==null||e.removeEventListener("transitionstart",this),h(this,Je,null)}attributeChangedCallback(e,r,o){super.attributeChangedCallback(e,r,o),r!=o&&(e===a.MEDIA_CURRENT_TIME||e===a.MEDIA_PAUSED||e===a.MEDIA_ENDED||e===a.MEDIA_LOADING||e===a.MEDIA_DURATION||e===a.MEDIA_SEEKABLE?(s(this,ye).update({start:qr(this),duration:this.mediaSeekableEnd-this.mediaSeekableStart,playbackRate:this.mediaPlaybackRate}),s(this,Gt).call(this),Ll(this)):e===a.MEDIA_BUFFERED&&this.updateBufferedBar(),(e===a.MEDIA_DURATION||e===a.MEDIA_SEEKABLE)&&(this.mediaChaptersCues=s(this,et),this.updateBar()))}get mediaChaptersCues(){return s(this,et)}set mediaChaptersCues(e){var r;h(this,et,e),this.updateSegments((r=s(this,et))==null?void 0:r.map(o=>({start:qr(this,o.startTime),end:qr(this,o.endTime)})))}get mediaPaused(){return y(this,a.MEDIA_PAUSED)}set mediaPaused(e){S(this,a.MEDIA_PAUSED,e)}get mediaLoading(){return y(this,a.MEDIA_LOADING)}set mediaLoading(e){S(this,a.MEDIA_LOADING,e)}get mediaDuration(){return P(this,a.MEDIA_DURATION)}set mediaDuration(e){H(this,a.MEDIA_DURATION,e)}get mediaCurrentTime(){return P(this,a.MEDIA_CURRENT_TIME)}set mediaCurrentTime(e){H(this,a.MEDIA_CURRENT_TIME,e)}get mediaPlaybackRate(){return P(this,a.MEDIA_PLAYBACK_RATE,1)}set mediaPlaybackRate(e){H(this,a.MEDIA_PLAYBACK_RATE,e)}get mediaBuffered(){let e=this.getAttribute(a.MEDIA_BUFFERED);return e?e.split(" ").map(r=>r.split(":").map(o=>+o)):[]}set mediaBuffered(e){if(!e){this.removeAttribute(a.MEDIA_BUFFERED);return}let r=e.map(o=>o.join(":")).join(" ");this.setAttribute(a.MEDIA_BUFFERED,r)}get mediaSeekable(){let e=this.getAttribute(a.MEDIA_SEEKABLE);if(e)return e.split(":").map(r=>+r)}set mediaSeekable(e){if(e==null){this.removeAttribute(a.MEDIA_SEEKABLE);return}this.setAttribute(a.MEDIA_SEEKABLE,e.join(":"))}get mediaSeekableEnd(){var r;let[,e=this.mediaDuration]=(r=this.mediaSeekable)!=null?r:[];return e}get mediaSeekableStart(){var r;let[e=0]=(r=this.mediaSeekable)!=null?r:[];return e}get mediaPreviewImage(){return D(this,a.MEDIA_PREVIEW_IMAGE)}set mediaPreviewImage(e){R(this,a.MEDIA_PREVIEW_IMAGE,e)}get mediaPreviewTime(){return P(this,a.MEDIA_PREVIEW_TIME)}set mediaPreviewTime(e){H(this,a.MEDIA_PREVIEW_TIME,e)}get mediaEnded(){return y(this,a.MEDIA_ENDED)}set mediaEnded(e){S(this,a.MEDIA_ENDED,e)}updateBar(){super.updateBar(),this.updateBufferedBar(),this.updateCurrentBox()}updateBufferedBar(){var n;let e=this.mediaBuffered;if(!e.length)return;let r;if(this.mediaEnded)r=1;else{let d=this.mediaCurrentTime,[,c=this.mediaSeekableStart]=(n=e.find(([m,b])=>m<=d&&d<=b))!=null?n:[];r=qr(this,c)}let{style:o}=F(this.shadowRoot,"#buffered");o.setProperty("width",`${r*100}%`)}updateCurrentBox(){if(!this.shadowRoot.querySelector('slot[name="current"]').assignedElements().length)return;let r=F(this.shadowRoot,"#current-rail"),o=F(this.shadowRoot,'[part~="current-box"]'),n=f(this,ji,wo).call(this,s(this,Qi)),d=f(this,zi,Co).call(this,n,this.range.valueAsNumber),c=f(this,Zi,Oo).call(this,n,this.range.valueAsNumber);r.style.transform=`translateX(${d})`,r.style.setProperty("--_range-width",`${n.range.width}`),o.style.setProperty("--_box-shift",`${c}`),o.style.setProperty("--_box-width",`${n.box.width}px`),o.style.setProperty("visibility","initial")}handleEvent(e){switch(super.handleEvent(e),e.type){case"input":f(this,zr,aa).call(this);break;case"pointermove":f(this,jr,sa).call(this,e);break;case"pointerup":s(this,tt)&&h(this,tt,!1);break;case"pointerdown":h(this,tt,!0);break;case"pointerleave":f(this,qt,Wr).call(this,null);break;case"transitionstart":me(e.target,this)&&setTimeout(()=>s(this,Gt).call(this),0);break}}};Je=new WeakMap,ye=new WeakMap,Wi=new WeakMap,Bt=new WeakMap,Yi=new WeakMap,Qi=new WeakMap,Vt=new WeakMap,Kt=new WeakMap,et=new WeakMap,tt=new WeakMap,Gt=new WeakMap,Yr=new WeakSet,na=function(){return this.isConnected&&!this.mediaPaused&&!this.mediaLoading&&!this.mediaEnded&&this.mediaSeekableEnd>0&&mr(this)},Qr=new WeakMap,ji=new WeakSet,wo=function(e){var b;let o=((b=this.getAttribute("bounds")?Me(this,`#${this.getAttribute("bounds")}`):this.parentElement)!=null?b:this).getBoundingClientRect(),n=this.range.getBoundingClientRect(),d=e.offsetWidth,c=-(n.left-o.left-d/2),m=o.right-n.left-d/2;return{box:{width:d,min:c,max:m},bounds:o,range:n}},zi=new WeakSet,Co=function(e,r){let o=`${r*100}%`,{width:n,min:d,max:c}=e.box;if(!n)return o;if(Number.isNaN(d)||(o=`max(${`calc(1 / var(--_range-width) * 100 * ${d}% + var(--media-box-padding-left))`}, ${o})`),!Number.isNaN(c)){let b=`calc(1 / var(--_range-width) * 100 * ${c}% - var(--media-box-padding-right))`;o=`min(${o}, ${b})`}return o},Zi=new WeakSet,Oo=function(e,r){let{width:o,min:n,max:d}=e.box,c=r*e.range.width;if(c<n+s(this,Vt)){let m=e.range.left-e.bounds.left-s(this,Vt);return`${c-o/2+m}px`}if(c>d-s(this,Kt)){let m=e.bounds.right-e.range.right-s(this,Kt);return`${c+o/2-m-e.range.width}px`}return 0},jr=new WeakSet,sa=function(e){let r=[...s(this,Wi)].some(T=>e.composedPath().includes(T));if(!this.dragging&&(r||!e.composedPath().includes(this))){f(this,qt,Wr).call(this,null);return}let o=this.mediaSeekableEnd;if(!o)return;let n=F(this.shadowRoot,"#preview-rail"),d=F(this.shadowRoot,'[part~="preview-box"]'),c=f(this,ji,wo).call(this,s(this,Yi)),m=(e.clientX-c.range.left)/c.range.width;m=Math.max(0,Math.min(1,m));let b=f(this,zi,Co).call(this,c,m),A=f(this,Zi,Oo).call(this,c,m);n.style.transform=`translateX(${b})`,n.style.setProperty("--_range-width",`${c.range.width}`),d.style.setProperty("--_box-shift",`${A}`),d.style.setProperty("--_box-width",`${c.box.width}px`);let M=Math.round(s(this,Bt))-Math.round(m*o);Math.abs(M)<1&&m>.01&&m<.99||(h(this,Bt,m*o),f(this,qt,Wr).call(this,s(this,Bt)))},qt=new WeakSet,Wr=function(e){this.dispatchEvent(new l.CustomEvent(E.MEDIA_PREVIEW_REQUEST,{composed:!0,bubbles:!0,detail:e}))},zr=new WeakSet,aa=function(){s(this,ye).stop();let e=oa(this);this.dispatchEvent(new l.CustomEvent(E.MEDIA_SEEK_REQUEST,{composed:!0,bubbles:!0,detail:e}))},$t.shadowRootOptions={mode:"open"},$t.getContainerTemplateHTML=kl;l.customElements.get("media-time-range")||l.customElements.define("media-time-range",$t);var da=$t;var Rl=1,Dl=i=>i.mediaMuted?0:i.mediaVolume,xl=i=>`${Math.round(i*100)}%`,Xi,Zr=class extends se{constructor(){super(...arguments);u(this,Xi,()=>{let e=this.range.value,r=new l.CustomEvent(E.MEDIA_VOLUME_REQUEST,{composed:!0,bubbles:!0,detail:e});this.dispatchEvent(r)})}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_VOLUME,a.MEDIA_MUTED,a.MEDIA_VOLUME_UNAVAILABLE]}connectedCallback(){super.connectedCallback(),this.range.setAttribute("aria-label",g("volume")),this.range.addEventListener("input",s(this,Xi))}disconnectedCallback(){this.range.removeEventListener("input",s(this,Xi)),super.disconnectedCallback()}attributeChangedCallback(e,r,o){super.attributeChangedCallback(e,r,o),(e===a.MEDIA_VOLUME||e===a.MEDIA_MUTED)&&(this.range.valueAsNumber=Dl(this),this.range.setAttribute("aria-valuetext",xl(this.range.valueAsNumber)),this.updateBar())}get mediaVolume(){return P(this,a.MEDIA_VOLUME,Rl)}set mediaVolume(e){H(this,a.MEDIA_VOLUME,e)}get mediaMuted(){return y(this,a.MEDIA_MUTED)}set mediaMuted(e){S(this,a.MEDIA_MUTED,e)}get mediaVolumeUnavailable(){return D(this,a.MEDIA_VOLUME_UNAVAILABLE)}set mediaVolumeUnavailable(e){R(this,a.MEDIA_VOLUME_UNAVAILABLE,e)}};Xi=new WeakMap;l.customElements.get("media-volume-range")||l.customElements.define("media-volume-range",Zr);var la=Zr;function wl(i){return`
      <style>
        :host {
          min-width: 4ch;
          padding: var(--media-button-padding, var(--media-control-padding, 10px 5px));
          width: 100%;
          display: grid;
          grid-template-columns: 1fr auto;
          gap: 1rem;
          font-weight: var(--media-button-font-weight, normal);
        }

        #checked-indicator {
          display: none;
        }

        :host([${a.MEDIA_LOOP}]) #checked-indicator {
          display: block;
        }
      </style>
      
      <span id="icon">
     </span>

      <div id="checked-indicator">
        <svg aria-hidden="true" viewBox="0 1 24 24" part="checked-indicator indicator">
          <path d="m10 15.17 9.193-9.191 1.414 1.414-10.606 10.606-6.364-6.364 1.414-1.414 4.95 4.95Z"/>
        </svg>
      </div>
    `}function Cl(){return g("Loop")}var Wt=class extends U{constructor(){super(...arguments);this.container=null}static get observedAttributes(){return[...super.observedAttributes,a.MEDIA_LOOP]}connectedCallback(){var e;super.connectedCallback(),this.setAttribute("role","switch"),this.container=((e=this.shadowRoot)==null?void 0:e.querySelector("#icon"))||null,this.container&&(this.container.textContent=g("Loop"))}attributeChangedCallback(e,r,o){super.attributeChangedCallback(e,r,o),e===a.MEDIA_LOOP&&this.container&&this.setAttribute("aria-checked",this.mediaLoop?"true":"false")}get mediaLoop(){return y(this,a.MEDIA_LOOP)}set mediaLoop(e){S(this,a.MEDIA_LOOP,e)}handleClick(){let e=!this.mediaLoop,r=new l.CustomEvent(E.MEDIA_LOOP_REQUEST,{composed:!0,bubbles:!0,detail:e});this.dispatchEvent(r)}};Wt.getSlotTemplateHTML=wl,Wt.getTooltipContentHTML=Cl;l.customElements.get("media-loop-button")||l.customElements.define("media-loop-button",Wt);var ca=Wt;return ya(Ol);})();
//# sourceMappingURL=index.js.map
